// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 事务性 Outbox 投递端（2026-10-28 修复「订单已写、事件未发」）
//!
//! 写入端在订单事务内插入 `outbox_event`（见 `order_service::create`）；
//! 本模块负责多实例安全地抢占并投递事件：
//!
//! - 抢占：`SELECT ... FOR UPDATE SKIP LOCKED` + 租约，避免同毫秒重复认领；
//! - 成功：`mark_delivered` 置 status=1；
//! - 失败：`mark_failed` 指数退避重试，超过上限置 status=2（需人工介入）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::db::acquire_cancel_safe;
use sz_rust_core::orm::{Pool, Value};

/// 待投递事件
#[derive(Debug, Clone)]
pub struct OutboxEvent {
    /// `outbox_event` 主键
    pub id: i64,
    /// 聚合类型（如 `order`）
    pub aggregate_type: String,
    /// 聚合根 ID
    pub aggregate_id: String,
    /// 事件类型（如 `order.created`）
    pub event_type: String,
    /// 事件载荷（JSON 字符串）
    pub payload: String,
    /// 已投递尝试次数（含本次）
    pub attempts: i32,
}

/// 投递函数（由业务方注入，例如发 MQTT / Kafka）
pub type DeliverFn = Arc<
    dyn Fn(OutboxEvent) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync,
>;

const MAX_ATTEMPTS: i32 = 10;

/// 记录底层错误并返回对外错误串（不回传数据库细节）
fn log_err(e: impl std::fmt::Display, ctx: &str) -> String {
    tracing::error!(error = %e, "{ctx}");
    "投递失败".to_string()
}

/// 取列文本值
///
/// `Value` 的 `Display` 是 SQL 字面量风格（字符串/JSON 带单引号），不能用 `to_string()`；
/// `as_str()` 只覆盖 `String`/`Decimal`，BLOB/TEXT 列可能落在 `Value::Bytes`，需单独解码。
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::Json(s)) => s.clone(),
        Some(Value::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        Some(v) => v.as_str().map(str::to_string).unwrap_or_default(),
        None => String::new(),
    }
}

/// 抢占一批待投递事件（租约期内不会被其他实例重复领取）
pub async fn claim_pending(
    pool: &Pool,
    limit: i64,
    lease_secs: i64,
) -> Result<Vec<OutboxEvent>, String> {
    let mut conn = acquire_cancel_safe(pool)
        .await
        .map_err(|e| log_err(e, "outbox 抢占获取连接失败"))?;

    conn.begin_transaction()
        .await
        .map_err(|e| log_err(e, "outbox 抢占开启事务失败"))?;

    let claim = async {
        // payload 是 JSON 列：sz-orm 的 MySQL 解码路径（ColType::Json 走
        // `try_get::<String>`）在二进制协议下取不到值且静默返回 Value::Null，
        // 投递出去就是空载荷。SQL 侧 CAST 成 CHAR 让列以 VARCHAR 文本返回。
        let rows = conn
            .query_with_params(
                "SELECT id, aggregate_type, aggregate_id, event_type, \
                        CAST(payload AS CHAR) AS payload, attempts \
                 FROM outbox_event \
                 WHERE status = 0 \
                   AND (next_retry_at IS NULL OR next_retry_at <= NOW(3)) \
                   AND (locked_until IS NULL OR locked_until <= NOW(3)) \
                 ORDER BY id LIMIT ? FOR UPDATE SKIP LOCKED",
                &[Value::I64(limit)],
            )
            .await
            .map_err(|e| log_err(e, "outbox 抢占查询失败"))?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            if id <= 0 {
                continue;
            }
            conn.execute_with_params(
                "UPDATE outbox_event \
                 SET locked_until = DATE_ADD(NOW(3), INTERVAL ? SECOND), attempts = attempts + 1 \
                 WHERE id = ?",
                &[Value::I64(lease_secs), Value::I64(id)],
            )
            .await
            .map_err(|e| log_err(e, "outbox 抢占更新租约失败"))?;

            events.push(OutboxEvent {
                id,
                aggregate_type: text_of(row.get("aggregate_type")),
                aggregate_id: text_of(row.get("aggregate_id")),
                event_type: text_of(row.get("event_type")),
                payload: text_of(row.get("payload")),
                attempts: row.get("attempts").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            });
        }
        Ok::<Vec<OutboxEvent>, String>(events)
    }
    .await;

    match claim {
        Ok(events) => {
            conn.commit()
                .await
                .map_err(|e| log_err(e, "outbox 抢占提交失败"))?;
            Ok(events)
        }
        Err(e) => {
            let _ = conn.rollback().await;
            Err(e)
        }
    }
}

/// 标记投递成功
pub async fn mark_delivered(pool: &Pool, id: i64) -> Result<(), String> {
    let mut conn = acquire_cancel_safe(pool)
        .await
        .map_err(|e| log_err(e, "outbox 标记成功获取连接失败"))?;
    conn.execute_with_params(
        "UPDATE outbox_event SET status = 1, locked_until = NULL, last_error = NULL WHERE id = ?",
        &[Value::I64(id)],
    )
    .await
    .map_err(|e| log_err(e, "outbox 标记成功失败"))?;
    Ok(())
}

/// 标记投递失败（指数退避；超过上限置 status=2 等待人工处理）
pub async fn mark_failed(pool: &Pool, id: i64, error: &str, attempts: i32) -> Result<(), String> {
    let mut conn = acquire_cancel_safe(pool)
        .await
        .map_err(|e| log_err(e, "outbox 标记失败获取连接失败"))?;

    let truncated: String = error.chars().take(512).collect();
    if attempts >= MAX_ATTEMPTS {
        conn.execute_with_params(
            "UPDATE outbox_event SET status = 2, locked_until = NULL, last_error = ? WHERE id = ?",
            &[Value::String(truncated), Value::I64(id)],
        )
        .await
        .map_err(|e| log_err(e, "outbox 标记死信失败"))?;
        return Ok(());
    }

    // 退避：2^attempts 秒，最大 300 秒
    let backoff = (1i64 << attempts.min(8)).min(300);
    conn.execute_with_params(
        "UPDATE outbox_event \
         SET status = 0, locked_until = NULL, last_error = ?, \
             next_retry_at = DATE_ADD(NOW(3), INTERVAL ? SECOND) \
         WHERE id = ?",
        &[
            Value::String(truncated),
            Value::I64(backoff),
            Value::I64(id),
        ],
    )
    .await
    .map_err(|e| log_err(e, "outbox 标记重试失败"))?;
    Ok(())
}

/// 执行一轮投递，返回处理数量
pub async fn dispatch_once(
    pool: &Pool,
    handler: &DeliverFn,
    limit: i64,
    lease_secs: i64,
) -> Result<usize, String> {
    let events = claim_pending(pool, limit, lease_secs).await?;
    let mut handled = 0;
    for event in events {
        let attempts = event.attempts;
        let id = event.id;
        match handler(event).await {
            Ok(()) => {
                mark_delivered(pool, id).await?;
                handled += 1;
            }
            Err(e) => {
                mark_failed(pool, id, &e, attempts).await?;
            }
        }
    }
    Ok(handled)
}

/// 启动后台投递循环（业务方需提供真实的投递函数）
pub fn spawn_dispatcher(
    pool: Pool,
    handler: DeliverFn,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match dispatch_once(&pool, &handler, 100, 30).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(count = n, "outbox 投递一轮完成"),
                Err(e) => tracing::warn!(error = %e, "outbox 投递一轮失败"),
            }
            tokio::time::sleep(interval).await;
        }
    })
}

/// 构造 HTTP Webhook 投递函数
///
/// 语义：下游返回 2xx 才算送达；其余（含超时、连接失败、非 2xx）一律返回 `Err`，
/// 交给 `mark_failed` 的指数退避与死信逻辑，避免"发出去就算成功"的假投递。
///
/// 载荷包含事件元数据与已解析的 `payload`；**不记录 payload 原文到日志**（可能含用户数据）。
pub fn webhook_handler(url: String) -> DeliverFn {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "outbox webhook 客户端构建失败，退回默认配置");
            reqwest::Client::new()
        });

    Arc::new(move |event: OutboxEvent| {
        let client = client.clone();
        let url = url.clone();
        Box::pin(async move {
            let payload = serde_json::from_str::<serde_json::Value>(&event.payload)
                .unwrap_or_else(|_| serde_json::Value::String(event.payload.clone()));
            let body = serde_json::json!({
                "id": event.id,
                "aggregate_type": event.aggregate_type,
                "aggregate_id": event.aggregate_id,
                "event_type": event.event_type,
                "payload": payload,
            });

            let response = client.post(&url).json(&body).send().await.map_err(|e| {
                // reqwest 的 Display 会带完整 URL（可能含 ?token=…），只记录错误类别
                let kind = if e.is_timeout() {
                    "timeout"
                } else if e.is_connect() {
                    "connect"
                } else {
                    "request"
                };
                tracing::warn!(event_id = event.id, kind, "outbox webhook 请求失败");
                format!("webhook 请求失败（{kind}）")
            })?;

            if response.status().is_success() {
                Ok(())
            } else {
                let status = response.status();
                tracing::warn!(event_id = event.id, status = status.as_u16(), "outbox webhook 返回非 2xx");
                Err(format!("webhook 返回 HTTP {}", status.as_u16()))
            }
        })
    })
}
