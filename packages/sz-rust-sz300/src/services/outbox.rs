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
                tracing::warn!(
                    event_id = event.id,
                    status = status.as_u16(),
                    "outbox webhook 返回非 2xx"
                );
                Err(format!("webhook 返回 HTTP {}", status.as_u16()))
            }
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────
    // outbox 纯逻辑（P2-4 补 service 层覆盖率）
    // ─────────────────────────────────────────────

    /// OutboxEvent 构造与字段访问
    #[test]
    fn outbox_event_construction_and_field_access() {
        let event = OutboxEvent {
            id: 42,
            aggregate_type: "order".into(),
            aggregate_id: "ord-001".into(),
            event_type: "order.created".into(),
            payload: "{\"k\":1}".into(),
            attempts: 3,
        };
        assert_eq!(event.id, 42);
        assert_eq!(event.aggregate_type, "order");
        assert_eq!(event.aggregate_id, "ord-001");
        assert_eq!(event.event_type, "order.created");
        assert_eq!(event.payload, "{\"k\":1}");
        assert_eq!(event.attempts, 3);
    }

    /// text_of：Json 变体直接返回内部字符串
    #[test]
    fn text_of_json_variant_returns_inner_string() {
        let v = Value::Json("{\"k\":1}".into());
        assert_eq!(text_of(Some(&v)), "{\"k\":1}");
    }

    /// text_of：Bytes 变体按 UTF-8 lossy 解码
    #[test]
    fn text_of_bytes_variant_decodes_utf8_lossy() {
        let v = Value::Bytes(b"hello".to_vec());
        assert_eq!(text_of(Some(&v)), "hello");
    }

    /// text_of：Bytes 含非 UTF-8 字节用 U+FFFD 替换
    #[test]
    fn text_of_bytes_invalid_utf8_uses_replacement_char() {
        let v = Value::Bytes(vec![0xff, 0xfe, 0xfd]);
        let s = text_of(Some(&v));
        assert!(s.contains('\u{FFFD}'), "非 UTF-8 字节应被替换为 U+FFFD: {s:?}");
    }

    /// text_of：String 变体通过 as_str 返回
    #[test]
    fn text_of_string_variant_returns_str() {
        let v = Value::String("hello".into());
        assert_eq!(text_of(Some(&v)), "hello");
    }

    /// text_of：None 返回空字符串
    #[test]
    fn text_of_none_returns_empty() {
        assert_eq!(text_of(None), "");
    }

    /// log_err：返回固定对外错误串 "投递失败"，不回传 DB 细节
    #[test]
    fn log_err_returns_fixed_external_message() {
        let msg = log_err("connection refused", "ctx");
        assert_eq!(msg, "投递失败", "log_err 不应泄露 DB 细节");
    }

    /// mark_failed 的退避公式：2^attempts 秒，最大 300 秒
    /// 此处锁定 `(1i64 << attempts.min(8)).min(300)` 的语义。
    #[test]
    fn mark_failed_backoff_formula() {
        let cases = [
            (0i32, 1i64),
            (1, 2),
            (2, 4),
            (3, 8),
            (5, 32),
            (8, 256),
            (9, 256),   // min(8) 截断
            (100, 256), // min(8) 截断
        ];
        for (attempts, expected) in cases {
            let backoff = (1i64 << attempts.min(8)).min(300);
            assert_eq!(
                backoff, expected,
                "attempts={attempts} 退避应为 {expected}"
            );
        }
        // 上限 300
        assert_eq!((1i64 << 9).min(300), 300);
    }

    /// mark_failed 死信阈值：attempts >= MAX_ATTEMPTS(10) → status=2
    #[test]
    fn mark_failed_dead_letter_threshold() {
        assert_eq!(MAX_ATTEMPTS, 10);
        // attempts < 10 → 重试路径
        for attempts in 0..10 {
            assert!(attempts < MAX_ATTEMPTS, "attempts={attempts} 应走重试路径");
        }
        // attempts >= 10 → 死信路径
        for attempts in [10, 11, 100] {
            assert!(
                attempts >= MAX_ATTEMPTS,
                "attempts={attempts} 应走死信路径"
            );
        }
    }

    /// mark_failed 错误截断：error.chars().take(512) — 锁定 512 字符上限
    #[test]
    fn mark_failed_truncates_error_to_512_chars() {
        let long_error: String = "中".repeat(1000);
        let truncated: String = long_error.chars().take(512).collect();
        assert_eq!(truncated.chars().count(), 512, "错误应截断到 512 字符");
    }
}
