// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! DB 队列后端（T006）
//!
//! 将既有 `JobQueue` 的数据库操作重构为 `QueueBackend` trait 实现。
//! 使用 `sz_jobs` 表持久化，`FOR UPDATE SKIP LOCKED` 原子抢占。

use std::sync::Arc;

use async_trait::async_trait;
use sz_orm_core::{Pool, Value};

use super::{
    backoff_delay_ms, now_ms, row_to_job, Job, JobErrorKind, JobQueueConfig, JobQueueError,
    QueueBackend, QueueSnapshot, SCHEMA_SQL, STATUS_DEAD, STATUS_PENDING, STATUS_RUNNING,
    STATUS_SUCCEEDED,
};

/// DB 队列后端（基于 `sz_jobs` 表）
pub struct DbQueueBackend {
    pool: Arc<Pool>,
}

impl DbQueueBackend {
    /// 创建 DB 后端
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    /// 底层连接池引用
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }
}

#[async_trait]
impl QueueBackend for DbQueueBackend {
    async fn init_schema(&self) -> Result<(), JobQueueError> {
        let mut conn = self.pool.acquire().await?;
        conn.execute(SCHEMA_SQL).await?;
        Ok(())
    }

    async fn push(
        &self,
        kind: &str,
        payload: serde_json::Value,
        dedupe_key: Option<&str>,
        run_after: i64,
    ) -> Result<u64, JobQueueError> {
        let payload_str = serde_json::to_string(&payload)?;
        let now = now_ms();
        let mut conn = self.pool.acquire().await?;
        conn.execute_with_params(
            "INSERT INTO sz_jobs (kind, payload, status, attempts, run_after, dedupe_key, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE id = LAST_INSERT_ID(id)",
            &[
                Value::String(kind.into()),
                Value::String(payload_str),
                Value::String(STATUS_PENDING.into()),
                Value::I64(run_after),
                dedupe_key.map_or(Value::Null, |k| Value::String(k.into())),
                Value::I64(now),
                Value::I64(now),
            ],
        )
        .await?;
        let rows = conn
            .query_with_params("SELECT LAST_INSERT_ID() AS id", &[])
            .await?;
        rows.first()
            .and_then(|r| r.get("id"))
            .and_then(Value::as_i64)
            .map(|v| v as u64)
            .ok_or_else(|| JobQueueError::InvalidRow("LAST_INSERT_ID() 返回空".into()))
    }

    async fn pop(&self, config: &JobQueueConfig) -> Result<Vec<Job>, JobQueueError> {
        let now = now_ms();
        let locked_until = now + config.lease_seconds as i64 * 1000;
        let mut conn = self.pool.acquire().await?;
        conn.begin_transaction().await?;
        let rows = conn
            .query_with_params(
                "SELECT id FROM sz_jobs WHERE status = ? AND run_after <= ? \
                 ORDER BY created_at LIMIT ? FOR UPDATE SKIP LOCKED",
                &[
                    Value::String(STATUS_PENDING.into()),
                    Value::I64(now),
                    Value::I64(config.batch_size as i64),
                ],
            )
            .await?;
        let ids: Vec<Value> = rows
            .iter()
            .filter_map(|r| r.get("id").and_then(Value::as_i64).map(Value::I64))
            .collect();
        if !ids.is_empty() {
            let placeholders = vec!["?"; ids.len()].join(",");
            let mut params = Vec::with_capacity(ids.len() + 3);
            params.push(Value::String(STATUS_RUNNING.into()));
            params.push(Value::I64(locked_until));
            params.push(Value::I64(now));
            params.extend(ids);
            conn.execute_with_params(
                &format!(
                    "UPDATE sz_jobs SET status = ?, locked_until = ?, attempts = attempts + 1, updated_at = ? \
                     WHERE id IN ({placeholders})"
                ),
                &params,
            )
            .await?;
        }
        conn.commit().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, kind, payload, status, attempts, run_after, last_error, dedupe_key, created_at \
                 FROM sz_jobs WHERE status = ? AND locked_until = ? ORDER BY created_at",
                &[Value::String(STATUS_RUNNING.into()), Value::I64(locked_until)],
            )
            .await?;
        rows.into_iter().map(row_to_job).collect()
    }

    async fn ack(&self, job_id: u64) -> Result<(), JobQueueError> {
        let mut conn = self.pool.acquire().await?;
        conn.execute_with_params(
            "UPDATE sz_jobs SET status = ?, locked_until = NULL, updated_at = ? WHERE id = ?",
            &[
                Value::String(STATUS_SUCCEEDED.into()),
                Value::I64(now_ms()),
                Value::I64(job_id as i64),
            ],
        )
        .await?;
        Ok(())
    }

    async fn fail(
        &self,
        job_id: u64,
        attempts: u32,
        error: &str,
        kind: JobErrorKind,
        config: &JobQueueConfig,
    ) -> Result<(), JobQueueError> {
        let now = now_ms();
        let (status, run_after) = match kind {
            JobErrorKind::Temporary if attempts <= config.max_attempts => (
                STATUS_PENDING,
                now + backoff_delay_ms(config, attempts) as i64,
            ),
            _ => (STATUS_DEAD, now),
        };
        let mut conn = self.pool.acquire().await?;
        conn.execute_with_params(
            "UPDATE sz_jobs SET status = ?, run_after = ?, locked_until = NULL, last_error = ?, updated_at = ? WHERE id = ?",
            &[
                Value::String(status.into()),
                Value::I64(run_after),
                Value::String(error.into()),
                Value::I64(now),
                Value::I64(job_id as i64),
            ],
        )
        .await?;
        Ok(())
    }

    async fn retry_dead(&self, job_id: u64) -> Result<(), JobQueueError> {
        let now = now_ms();
        let mut conn = self.pool.acquire().await?;
        conn.execute_with_params(
            "UPDATE sz_jobs SET status = ?, run_after = ?, locked_until = NULL, updated_at = ? WHERE id = ? AND status = ?",
            &[
                Value::String(STATUS_PENDING.into()),
                Value::I64(now),
                Value::I64(now),
                Value::I64(job_id as i64),
                Value::String(STATUS_DEAD.into()),
            ],
        )
        .await?;
        Ok(())
    }

    async fn snapshot(&self) -> Result<QueueSnapshot, JobQueueError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query("SELECT status, COUNT(*) AS cnt FROM sz_jobs GROUP BY status")
            .await?;
        let mut snap = QueueSnapshot::default();
        for row in rows {
            let status = row.get("status").and_then(Value::as_str).unwrap_or("");
            let cnt = row.get("cnt").and_then(Value::as_i64).unwrap_or(0).max(0) as u64;
            match status {
                STATUS_PENDING => snap.pending = cnt,
                STATUS_RUNNING => snap.running = cnt,
                STATUS_SUCCEEDED => snap.succeeded = cnt,
                STATUS_DEAD => snap.dead = cnt,
                _ => {}
            }
        }
        let rows = conn
            .query("SELECT MIN(run_after) AS oldest FROM sz_jobs WHERE status = 'pending'")
            .await?;
        if let Some(oldest) = rows
            .first()
            .and_then(|r| r.get("oldest"))
            .and_then(Value::as_i64)
        {
            snap.oldest_pending_seconds = ((now_ms() - oldest).max(0) / 1000) as u64;
        }
        Ok(snap)
    }

    async fn reclaim_stale(&self, config: &JobQueueConfig) -> Result<(), JobQueueError> {
        let now = now_ms();
        let lease_deadline = now - config.lease_seconds as i64 * 1000;
        let mut conn = self.pool.acquire().await?;
        conn.execute_with_params(
            "UPDATE sz_jobs SET status = ?, locked_until = NULL, updated_at = ? \
             WHERE status = ? AND locked_until < ?",
            &[
                Value::String(STATUS_PENDING.into()),
                Value::I64(now),
                Value::String(STATUS_RUNNING.into()),
                Value::I64(lease_deadline),
            ],
        )
        .await?;
        Ok(())
    }
}
