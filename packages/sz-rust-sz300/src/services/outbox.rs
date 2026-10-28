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

use sz_rust_core::orm::{Pool, Value};

/// 待投递事件
#[derive(Debug, Clone)]
pub struct OutboxEvent {
    pub id: i64,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_type: String,
    pub payload: String,
    pub attempts: i32,
}

/// 投递函数（由业务方注入，例如发 MQTT / Kafka）
pub type DeliverFn = Arc<
    dyn Fn(OutboxEvent) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync,
>;

const MAX_ATTEMPTS: i32 = 10;

fn err<T>(e: impl std::fmt::Display, ctx: &str) -> Result<T, String> {
    tracing::error!(error = %e, "{ctx}");
    Err("投递失败".to_string())
}

/// 抢占一批待投递事件（租约期内不会被其他实例重复领取）
pub async fn claim_pending(
    pool: &Pool,
    limit: i64,
    lease_secs: i64,
) -> Result<Vec<OutboxEvent>, String> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| err::<()>(e, "outbox 抢占获取连接失败").unwrap_err())?;

    conn.begin_transaction()
        .await
        .map_err(|e| err::<()>(e, "outbox 抢占开启事务失败").unwrap_err())?;

    let claim = async {
        let rows = conn
            .query_with_params(
                "SELECT id, aggregate_type, aggregate_id, event_type, payload, attempts \
                 FROM outbox_event \
                 WHERE status = 0 \
                   AND (next_retry_at IS NULL OR next_retry_at <= NOW(3)) \
                   AND (locked_until IS NULL OR locked_until <= NOW(3)) \
                 ORDER BY id LIMIT ? FOR UPDATE SKIP LOCKED",
                &[Value::I64(limit)],
            )
            .await
            .map_err(|e| err::<()>(e, "outbox 抢占查询失败").unwrap_err())?;

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
            .map_err(|e| err::<()>(e, "outbox 抢占更新租约失败").unwrap_err())?;

            events.push(OutboxEvent {
                id,
                aggregate_type: row
                    .get("aggregate_type")
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                aggregate_id: row
                    .get("aggregate_id")
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                event_type: row
                    .get("event_type")
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                payload: row
                    .get("payload")
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
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
                .map_err(|e| err::<()>(e, "outbox 抢占提交失败").unwrap_err())?;
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
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| err::<()>(e, "outbox 标记成功获取连接失败").unwrap_err())?;
    conn.execute_with_params(
        "UPDATE outbox_event SET status = 1, locked_until = NULL, last_error = NULL WHERE id = ?",
        &[Value::I64(id)],
    )
    .await
    .map_err(|e| err::<()>(e, "outbox 标记成功失败").unwrap_err())?;
    Ok(())
}

/// 标记投递失败（指数退避；超过上限置 status=2 等待人工处理）
pub async fn mark_failed(pool: &Pool, id: i64, error: &str, attempts: i32) -> Result<(), String> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| err::<()>(e, "outbox 标记失败获取连接失败").unwrap_err())?;

    let truncated: String = error.chars().take(512).collect();
    if attempts >= MAX_ATTEMPTS {
        conn.execute_with_params(
            "UPDATE outbox_event SET status = 2, locked_until = NULL, last_error = ? WHERE id = ?",
            &[Value::String(truncated), Value::I64(id)],
        )
        .await
        .map_err(|e| err::<()>(e, "outbox 标记死信失败").unwrap_err())?;
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
    .map_err(|e| err::<()>(e, "outbox 标记重试失败").unwrap_err())?;
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
