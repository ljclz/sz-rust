// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Redis 队列后端（T007）
//!
//! 使用 Redis LIST（LPUSH/RPOP）存储就绪 Job，ZSET 存储延迟 Job
//! （score = available_at），后台定时将到期延迟 Job 从 ZSET 移到 LIST。
//!
//! ## Redis 键布局
//!
//! | 键 | 类型 | 说明 |
//! |----|------|------|
//! | `{prefix}:ready` | LIST | 就绪 Job ID 队列 |
//! | `{prefix}:delayed` | ZSET | 延迟 Job（score = run_afterEPOCHms） |
//! | `{prefix}:dead` | LIST | 死信 Job ID 队列 |
//! | `{prefix}:job:{id}` | STRING | Job JSON 数据 |
//! | `{prefix}:counter` | STRING | 自增 ID 计数器 |
//! | `{prefix}:dedupe:{kind}` | HASH | dedupe_key → job_id 映射 |
//! | `{prefix}:succeeded` | STRING | 成功计数器 |

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use super::{
    backoff_delay_ms, now_ms, Job, JobErrorKind, JobQueueConfig, JobQueueError, JobStatus,
    QueueBackend, QueueSnapshot,
};

/// Redis 队列后端
pub struct RedisQueueBackend {
    conn: redis::aio::ConnectionManager,
    prefix: String,
}

/// Redis 中存储的 Job 数据（序列化用）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RedisJob {
    id: u64,
    kind: String,
    payload: serde_json::Value,
    attempts: u32,
    run_after: i64,
    last_error: Option<String>,
    dedupe_key: Option<String>,
    created_at: i64,
}

impl RedisQueueBackend {
    /// 创建 Redis 后端
    ///
    /// `prefix` 为键前缀（如 `"sz_jobs"`），多实例隔离用不同前缀。
    pub fn new(conn: redis::aio::ConnectionManager, prefix: impl Into<String>) -> Self {
        Self {
            conn,
            prefix: prefix.into(),
        }
    }

    fn ready_key(&self) -> String {
        format!("{}:ready", self.prefix)
    }

    fn delayed_key(&self) -> String {
        format!("{}:delayed", self.prefix)
    }

    fn dead_key(&self) -> String {
        format!("{}:dead", self.prefix)
    }

    fn job_key(&self, id: u64) -> String {
        format!("{}:job:{}", self.prefix, id)
    }

    fn counter_key(&self) -> String {
        format!("{}:counter", self.prefix)
    }

    fn dedupe_key(&self, kind: &str) -> String {
        format!("{}:dedupe:{}", self.prefix, kind)
    }

    fn succeeded_key(&self) -> String {
        format!("{}:succeeded", self.prefix)
    }

    /// 启动延迟 Job 调度器（后台定时将到期延迟 Job 从 ZSET 移到 LIST）
    ///
    /// 返回 JoinHandle，可 abort 停止调度。
    pub fn start_delayed_scheduler(self: &Arc<Self>, interval: Duration) -> JoinHandle<()> {
        let backend = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = backend.move_expired_delayed().await {
                    tracing::warn!(target: "sz_orm::jobs::redis", "move expired delayed failed: {e}");
                }
            }
        })
    }

    /// 将到期的延迟 Job 从 ZSET 移到就绪 LIST
    async fn move_expired_delayed(&self) -> Result<(), JobQueueError> {
        let now = now_ms();
        let mut conn = self.conn.clone();

        let expired: Vec<u64> = conn
            .zrangebyscore(self.delayed_key(), 0, now)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;

        for job_id in expired {
            let _: () = conn
                .zrem(self.delayed_key(), job_id)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
            let _: () = conn
                .lpush(self.ready_key(), job_id)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        }

        Ok(())
    }
}

#[async_trait]
impl QueueBackend for RedisQueueBackend {
    async fn init_schema(&self) -> Result<(), JobQueueError> {
        Ok(())
    }

    async fn push(
        &self,
        kind: &str,
        payload: serde_json::Value,
        dedupe_key: Option<&str>,
        run_after: i64,
    ) -> Result<u64, JobQueueError> {
        let mut conn = self.conn.clone();

        if let Some(dk) = dedupe_key {
            let dedupe_hash = self.dedupe_key(kind);
            let existing: Option<u64> = conn
                .hget(&dedupe_hash, dk)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
            if let Some(id) = existing {
                return Ok(id);
            }
        }

        let job_id: u64 = conn
            .incr(self.counter_key(), 1)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;

        let now = now_ms();
        let job = RedisJob {
            id: job_id,
            kind: kind.to_string(),
            payload,
            attempts: 0,
            run_after,
            last_error: None,
            dedupe_key: dedupe_key.map(str::to_string),
            created_at: now,
        };

        let job_json = serde_json::to_string(&job)?;
        let _: () = conn
            .set(self.job_key(job_id), &job_json)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;

        if let Some(dk) = dedupe_key {
            let _: () = conn
                .hset(self.dedupe_key(kind), dk, job_id)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        }

        if run_after > now {
            let _: () = conn
                .zadd(self.delayed_key(), job_id, run_after)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        } else {
            let _: () = conn
                .lpush(self.ready_key(), job_id)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        }

        Ok(job_id)
    }

    async fn pop(&self, config: &JobQueueConfig) -> Result<Vec<Job>, JobQueueError> {
        let mut conn = self.conn.clone();
        let mut jobs = Vec::with_capacity(config.batch_size as usize);

        for _ in 0..config.batch_size {
            let job_id: Option<u64> = conn
                .rpop(self.ready_key(), None)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;

            let job_id = match job_id {
                Some(id) => id,
                None => break,
            };

            let job_json: Option<String> = conn
                .get(self.job_key(job_id))
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;

            let job_json = match job_json {
                Some(s) => s,
                None => continue,
            };

            let rjob: RedisJob = serde_json::from_str(&job_json)?;
            jobs.push(Job {
                id: rjob.id,
                kind: rjob.kind,
                payload: rjob.payload,
                status: JobStatus::Running,
                attempts: rjob.attempts + 1,
                run_after: rjob.run_after,
                last_error: rjob.last_error,
                dedupe_key: rjob.dedupe_key,
                created_at: rjob.created_at,
            });
        }

        Ok(jobs)
    }

    async fn ack(&self, job_id: u64) -> Result<(), JobQueueError> {
        let mut conn = self.conn.clone();
        let _: () = conn
            .del(self.job_key(job_id))
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        let _: u64 = conn
            .incr(self.succeeded_key(), 1)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
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
        let mut conn = self.conn.clone();

        let job_json: Option<String> = conn
            .get(self.job_key(job_id))
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;

        let job_json = match job_json {
            Some(s) => s,
            None => return Ok(()),
        };

        let mut rjob: RedisJob = serde_json::from_str(&job_json)?;
        rjob.attempts = attempts;
        rjob.last_error = Some(error.to_string());

        let now = now_ms();
        let (is_dead, run_after) = match kind {
            JobErrorKind::Temporary if attempts <= config.max_attempts => {
                (false, now + backoff_delay_ms(config, attempts) as i64)
            }
            _ => (true, now),
        };

        if is_dead {
            let _: () = conn
                .lpush(self.dead_key(), job_id)
                .await
                .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        } else {
            rjob.run_after = run_after;
            if run_after > now {
                let job_json = serde_json::to_string(&rjob)?;
                let _: () = conn
                    .set(self.job_key(job_id), &job_json)
                    .await
                    .map_err(|e| JobQueueError::Backend(e.to_string()))?;
                let _: () = conn
                    .zadd(self.delayed_key(), job_id, run_after)
                    .await
                    .map_err(|e| JobQueueError::Backend(e.to_string()))?;
            } else {
                let job_json = serde_json::to_string(&rjob)?;
                let _: () = conn
                    .set(self.job_key(job_id), &job_json)
                    .await
                    .map_err(|e| JobQueueError::Backend(e.to_string()))?;
                let _: () = conn
                    .lpush(self.ready_key(), job_id)
                    .await
                    .map_err(|e| JobQueueError::Backend(e.to_string()))?;
            }
        }

        Ok(())
    }

    async fn retry_dead(&self, job_id: u64) -> Result<(), JobQueueError> {
        let mut conn = self.conn.clone();
        let _: () = conn
            .lrem(self.dead_key(), 1, job_id)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        let _: () = conn
            .lpush(self.ready_key(), job_id)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn snapshot(&self) -> Result<QueueSnapshot, JobQueueError> {
        let mut conn = self.conn.clone();
        let ready_len: u64 = conn
            .llen(self.ready_key())
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        let delayed_len: u64 = conn
            .zcard(self.delayed_key())
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        let dead_len: u64 = conn
            .llen(self.dead_key())
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        let succeeded: u64 = conn
            .get(self.succeeded_key())
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;

        let oldest: Option<Vec<u64>> = conn
            .zrange(self.delayed_key(), 0, 0)
            .await
            .map_err(|e| JobQueueError::Backend(e.to_string()))?;
        let oldest_pending_seconds = if let Some(ids) = oldest {
            if let Some(first_id) = ids.first() {
                let job_json: Option<String> = conn
                    .get(self.job_key(*first_id))
                    .await
                    .map_err(|e| JobQueueError::Backend(e.to_string()))?;
                if let Some(s) = job_json {
                    if let Ok(rjob) = serde_json::from_str::<RedisJob>(&s) {
                        ((now_ms() - rjob.run_after).max(0) / 1000) as u64
                    } else {
                        0
                    }
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        };

        Ok(QueueSnapshot {
            pending: ready_len + delayed_len,
            running: 0,
            dead: dead_len,
            succeeded,
            oldest_pending_seconds,
        })
    }

    async fn reclaim_stale(&self, _config: &JobQueueConfig) -> Result<(), JobQueueError> {
        Ok(())
    }
}
