//! 可靠任务队列（Reliable Job Queue）
//!
//! `tokio::spawn` 只解决并发，不解决可靠性：进程重启丢任务、失败无人重试、
//! 重试无上限打爆下游、重复执行产生副作用。本模块提供基于数据库表的
//! 持久化任务队列，对应工程实践要点：
//!
//! - **任务先变成数据**：`sz_jobs` 表（kind/payload/status/attempts/run_after/...）
//! - **状态机**：pending（含延迟重试）/ running（含租约）/ succeeded / dead（可重放）
//! - **领取靠数据库裁决**：事务内 `SELECT ... FOR UPDATE SKIP LOCKED` + UPDATE
//!   抢占（MySQL 8.0.1+），多实例 worker 安全，不重复领取
//! - **退避重试有上限**：指数退避 + 随机抖动，Temporary/Permanent 错误分类
//! - **幂等**：`dedupe_key` 唯一约束，重复入队返回已有任务，不重复投递
//! - **崩溃自愈**：`locked_until` 租约超时的 running 任务自动回收重跑
//! - **死信可查看可重放**：`queue_snapshot()` + `retry_dead()`
//! - **观测看队列健康**：pending/running/dead/最老 pending 等待时间
//!
//! 时间统一用 BIGINT 毫秒时间戳（UTC），避免 DATETIME 时区歧义。
//! SQL 全部参数化绑定（`execute_with_params` / `query_with_params`），
//! 列投影显式声明，无 `SELECT *`。
//!
//! # 并发领取的实现取舍（2026-08-15 修正）
//!
//! v1 曾用"单条 `UPDATE ... WHERE id IN (SELECT ...)` 抢占"，实测发现并发缺陷：
//! MySQL 默认 REPEATABLE READ 下，子查询是快照读（返回另一 worker 尚未提交的
//! pending 行），且 InnoDB 的 UPDATE 锁等待后不重新评估 WHERE（semi-consistent
//! read 仅 READ COMMITTED 启用），导致两个 worker 重复领取同一任务（集成测试
//! 实测 calls=2×）。修正为事务内 `SELECT ... FOR UPDATE SKIP LOCKED`——在锁定
//! 阶段就跳过他人已锁的行，是 MySQL/PostgreSQL 通用的标准做法。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sz_orm_core::{DbError, Pool, PoolError, Value};
use thiserror::Error;

/// 任务表名
pub const JOBS_TABLE: &str = "sz_jobs";

/// 建表 SQL（幂等，MySQL 方言；sz300 主数据源为 MySQL）
const SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS sz_jobs (
  id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
  kind VARCHAR(64) NOT NULL,
  payload TEXT NOT NULL,
  status VARCHAR(16) NOT NULL DEFAULT 'pending',
  attempts INT NOT NULL DEFAULT 0,
  run_after BIGINT NOT NULL,
  locked_until BIGINT NULL,
  last_error TEXT NULL,
  dedupe_key VARCHAR(255) NULL,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  UNIQUE KEY uq_sz_jobs_dedupe (kind, dedupe_key),
  KEY idx_sz_jobs_status_run_after (status, run_after)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4";

const STATUS_PENDING: &str = "pending";
const STATUS_RUNNING: &str = "running";
const STATUS_SUCCEEDED: &str = "succeeded";
const STATUS_DEAD: &str = "dead";

/// 任务状态机
///
/// - `Pending`：待执行；`run_after` 表达延迟与退避（未到时间不执行）
/// - `Running`：执行中；`locked_until` 为租约，超时后会被回收重新入队
/// - `Succeeded`：已完成
/// - `Dead`：永久失败（重试超限或 Permanent 错误），可人工重放
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// 待执行（含延迟重试）
    Pending,
    /// 执行中（租约内）
    Running,
    /// 已完成
    Succeeded,
    /// 永久失败，可重放
    Dead,
}

impl JobStatus {
    /// 状态 → 数据库字符串（pending/running/succeeded/dead）
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => STATUS_PENDING,
            JobStatus::Running => STATUS_RUNNING,
            JobStatus::Succeeded => STATUS_SUCCEEDED,
            JobStatus::Dead => STATUS_DEAD,
        }
    }

    /// 数据库字符串 → 状态
    pub fn parse_status(s: &str) -> Option<JobStatus> {
        match s {
            STATUS_PENDING => Some(JobStatus::Pending),
            STATUS_RUNNING => Some(JobStatus::Running),
            STATUS_SUCCEEDED => Some(JobStatus::Succeeded),
            STATUS_DEAD => Some(JobStatus::Dead),
            _ => None,
        }
    }
}

/// 任务错误分类：决定失败后是否重试
///
/// - `Temporary`：可重试（下游 503、数据库短暂故障、SMTP 不可用）
/// - `Permanent`：不可重试（参数缺失、用户不存在、模板配置错误）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobErrorKind {
    /// 临时失败，可重试
    Temporary,
    /// 永久失败，不应重试
    Permanent,
}

/// 任务处理器错误
#[derive(Debug, Error)]
pub enum JobError {
    /// 临时失败（将按退避策略重试）
    #[error("temporary job failure: {0}")]
    Temporary(String),
    /// 永久失败（进入死信）
    #[error("permanent job failure: {0}")]
    Permanent(String),
}

impl JobError {
    /// 错误分类（决定重试还是进死信）
    pub fn kind(&self) -> JobErrorKind {
        match self {
            JobError::Temporary(_) => JobErrorKind::Temporary,
            JobError::Permanent(_) => JobErrorKind::Permanent,
        }
    }
}

/// 任务处理器（注册到队列，按 `kind` 分发）
#[async_trait]
pub trait TaskHandler: Send + Sync + 'static {
    /// 处理任务；返回 `JobError::Temporary` 将退避重试，
    /// 返回 `JobError::Permanent` 直接进入死信
    async fn handle(&self, payload: &serde_json::Value) -> Result<(), JobError>;
}

/// 队列操作错误
#[derive(Debug, Error)]
pub enum JobQueueError {
    /// 数据库错误
    #[error("database error: {0}")]
    Db(#[from] DbError),
    /// 连接池错误
    #[error("pool error: {0}")]
    Pool(#[from] PoolError),
    /// 任务行数据非法
    #[error("invalid job row: {0}")]
    InvalidRow(String),
    /// 序列化错误
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// worker 配置
#[derive(Debug, Clone)]
pub struct JobQueueConfig {
    /// 每轮最多领取的任务数
    pub batch_size: u32,
    /// 轮询间隔
    pub poll_interval: Duration,
    /// 单任务最大重试次数（超过进入死信）
    pub max_attempts: u32,
    /// 指数退避基数（秒）：`base * 2^min(attempts, 6)`
    pub backoff_base_secs: u64,
    /// 退避上限（秒）
    pub backoff_cap_secs: u64,
    /// running 租约时长（秒）：worker 崩溃后超时回收
    pub lease_seconds: u64,
    /// 退避随机抖动比例（0~1），避免批量失败任务同时冲击下游
    pub jitter_ratio: f64,
    /// 单次 handler 执行超时
    pub handler_timeout: Duration,
}

impl Default for JobQueueConfig {
    fn default() -> Self {
        Self {
            batch_size: 10,
            poll_interval: Duration::from_secs(1),
            max_attempts: 8,
            backoff_base_secs: 1,
            backoff_cap_secs: 64,
            lease_seconds: 60,
            jitter_ratio: 0.3,
            handler_timeout: Duration::from_secs(30),
        }
    }
}

/// 任务（数据库行的内存表示）
#[derive(Debug, Clone)]
pub struct Job {
    /// 任务 ID
    pub id: u64,
    /// 任务类型（对应注册的 handler）
    pub kind: String,
    /// 任务负载（JSON）
    pub payload: serde_json::Value,
    /// 当前状态
    pub status: JobStatus,
    /// 已尝试次数（领取即 +1）
    pub attempts: u32,
    /// 最早可执行时间（毫秒时间戳，UTC）
    pub run_after: i64,
    /// 最近一次错误（用于死信排查）
    pub last_error: Option<String>,
    /// 幂等键（同 kind 下唯一）
    pub dedupe_key: Option<String>,
    /// 创建时间（毫秒时间戳，UTC）
    pub created_at: i64,
}

/// 队列健康快照（观测：看队列是否追得上生产，而非单任务成败）
#[derive(Debug, Clone, Copy, Default)]
pub struct QueueSnapshot {
    /// 待执行任务数
    pub pending: u64,
    /// 执行中任务数
    pub running: u64,
    /// 死信任务数
    pub dead: u64,
    /// 累计完成任务数
    pub succeeded: u64,
    /// 最老 pending 任务等待秒数（>300s 说明消费追不上生产）
    pub oldest_pending_seconds: u64,
}

/// 可靠任务队列
///
/// 基于 sz-orm `Pool` 实现，不绑定具体数据库后端（MySQL/PostgreSQL 均可，
/// 领取用单条 UPDATE 原子抢占，不依赖 `FOR UPDATE SKIP LOCKED` 方言）。
#[derive(Clone)]
pub struct JobQueue {
    pool: Arc<Pool>,
}

impl JobQueue {
    /// 创建任务队列
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    /// 底层连接池引用（观测/测试用）
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    /// 幂等建表（可安全重复调用）
    pub async fn init_schema(&self) -> Result<(), JobQueueError> {
        let mut conn = self.pool.acquire().await?;
        conn.execute(SCHEMA_SQL).await?;
        Ok(())
    }

    /// 入队任务（立即执行）。`dedupe_key` 同 kind 下重复时返回已存在任务 ID，不重复入队。
    pub async fn enqueue(
        &self,
        kind: &str,
        payload: serde_json::Value,
        dedupe_key: Option<&str>,
    ) -> Result<u64, JobQueueError> {
        self.enqueue_at(kind, payload, dedupe_key, now_ms()).await
    }

    /// 入队延迟任务（`delay` 后执行）——退避/定时不靠 worker sleep，靠 `run_after`
    pub async fn enqueue_delayed(
        &self,
        kind: &str,
        payload: serde_json::Value,
        dedupe_key: Option<&str>,
        delay: Duration,
    ) -> Result<u64, JobQueueError> {
        self.enqueue_at(
            kind,
            payload,
            dedupe_key,
            now_ms() + delay.as_millis() as i64,
        )
        .await
    }

    /// 入队核心：INSERT + 唯一约束幂等（重复返回已有 ID）
    async fn enqueue_at(
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

    /// 死信重放：将 dead 任务重新置为 pending（保留 attempts 与错误历史）
    pub async fn retry_dead(&self, job_id: u64) -> Result<(), JobQueueError> {
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

    /// 队列健康快照（pending/running/dead/最老等待/累计完成）
    pub async fn queue_snapshot(&self) -> Result<QueueSnapshot, JobQueueError> {
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

    /// 启动 worker：轮询领取 → 分发 handler → 成功/退避重试/死信。
    /// `shutdown` 为 true 时退出（优雅关闭）。
    pub async fn run_worker(
        &self,
        handlers: HashMap<String, Arc<dyn TaskHandler>>,
        config: JobQueueConfig,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), JobQueueError> {
        let mut interval = tokio::time::interval(config.poll_interval);
        loop {
            interval.tick().await;
            if *shutdown.borrow() {
                tracing::info!(target: "sz_orm::jobs", "job worker shutting down");
                return Ok(());
            }
            // 崩溃自愈：租约超时的 running 任务回收重跑
            if let Err(e) = self.reclaim_stale(&config).await {
                tracing::error!(target: "sz_orm::jobs", "reclaim stale jobs failed: {e}");
                continue;
            }
            let jobs = match self.claim_batch(&config).await {
                Ok(jobs) => jobs,
                Err(e) => {
                    tracing::error!(target: "sz_orm::jobs", "claim jobs failed: {e}");
                    continue;
                }
            };
            if jobs.is_empty() {
                // 队列健康观测（每轮无任务时仅 debug）
                if let Ok(snap) = self.queue_snapshot().await {
                    tracing::debug!(
                        target: "sz_orm::jobs",
                        "queue snapshot: pending={}, running={}, dead={}, oldest_pending_secs={}",
                        snap.pending, snap.running, snap.dead, snap.oldest_pending_seconds
                    );
                }
                continue;
            }
            for job in jobs {
                let handler = handlers.get(&job.kind);
                let outcome = match handler {
                    Some(h) => {
                        match tokio::time::timeout(config.handler_timeout, h.handle(&job.payload))
                            .await
                        {
                            Ok(Ok(())) => Ok(()),
                            Ok(Err(e)) => Err(e),
                            Err(_) => Err(JobError::Temporary(format!(
                                "handler timeout after {:?}",
                                config.handler_timeout
                            ))),
                        }
                    }
                    None => Err(JobError::Permanent(format!(
                        "no handler registered for kind '{}'",
                        job.kind
                    ))),
                };
                match outcome {
                    Ok(()) => {
                        self.mark_succeeded(job.id).await?;
                        tracing::debug!(target: "sz_orm::jobs", "job {} (kind={}) succeeded", job.id, job.kind);
                    }
                    Err(e) => {
                        self.handle_failure(
                            job.id,
                            job.attempts,
                            &e.to_string(),
                            e.kind(),
                            &config,
                        )
                        .await?;
                        tracing::warn!(
                            target: "sz_orm::jobs",
                            "job {} (kind={}) failed: {} (kind={:?}), attempts={}",
                            job.id, job.kind, e, e.kind(), job.attempts
                        );
                    }
                }
            }
        }
    }

    /// 崩溃自愈：将租约超时的 running 任务回收为 pending（不丢失、不卡死）
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

    /// 原子领取一批任务（多 worker 安全：SELECT FOR UPDATE SKIP LOCKED + 事务内 UPDATE）
    ///
    /// 为什么不用"单条 UPDATE 抢占"：MySQL 默认 REPEATABLE READ 下 InnoDB 的
    /// UPDATE 锁等待后不重新评估 WHERE（semi-consistent read 仅 READ COMMITTED
    /// 启用），并发 worker 会更新到另一 worker 已领取的行（实测重复执行 2x）。
    /// `FOR UPDATE SKIP LOCKED`（MySQL 8.0.1+）在锁定阶段就跳过他人已锁的行，
    /// 是官方推荐的多 worker 领取方式（同 PostgreSQL 的 SKIP LOCKED 语义）。
    async fn claim_batch(&self, config: &JobQueueConfig) -> Result<Vec<Job>, JobQueueError> {
        let now = now_ms();
        let locked_until = now + config.lease_seconds as i64 * 1000;
        let mut conn = self.pool.acquire().await?;
        conn.begin_transaction().await?;
        // 1. 锁定候选行：SKIP LOCKED 跳过其他 worker 已锁定的行（不等待）
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
        // 2. 事务内抢占（行已被本事务锁定，无竞争）；IN 占位符数量由代码生成，值全参数化
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
        // 3. 读回本 worker 领取的任务（按 locked_until 精确过滤，不捞其他 worker 的）
        let rows = conn
            .query_with_params(
                "SELECT id, kind, payload, status, attempts, run_after, last_error, dedupe_key, created_at \
                 FROM sz_jobs WHERE status = ? AND locked_until = ? ORDER BY created_at",
                &[Value::String(STATUS_RUNNING.into()), Value::I64(locked_until)],
            )
            .await?;
        rows.into_iter().map(row_to_job).collect()
    }

    /// 标记成功
    async fn mark_succeeded(&self, job_id: u64) -> Result<(), JobQueueError> {
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

    /// 失败处理：Temporary 且未超限 → 退避重试；否则 → 死信
    async fn handle_failure(
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
}

/// 指数退避（毫秒）：`base * 2^min(attempts, 6)`，封顶 `cap`，含随机抖动
///
/// `attempts` 为已尝试次数（领取时 +1 后的值）。
/// 抖动目的：批量失败任务不会同时恢复、同时冲击下游
pub fn backoff_delay_ms(config: &JobQueueConfig, attempts: u32) -> u64 {
    let exp = (attempts as i32).min(6);
    let delay_secs =
        (config.backoff_base_secs as f64 * 2f64.powi(exp)).min(config.backoff_cap_secs as f64);
    let jitter = delay_secs * config.jitter_ratio.clamp(0.0, 1.0) * rand::random::<f64>();
    ((delay_secs + jitter) * 1000.0) as u64
}

/// 当前毫秒时间戳（UTC）
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 行 → Job 转换（显式列投影读取）
fn row_to_job(row: HashMap<String, Value>) -> Result<Job, JobQueueError> {
    let id = row
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| JobQueueError::InvalidRow("id".into()))?;
    let kind = row
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| JobQueueError::InvalidRow("kind".into()))?
        .to_string();
    let payload_str = row
        .get("payload")
        .and_then(Value::as_str)
        .ok_or_else(|| JobQueueError::InvalidRow("payload".into()))?;
    let payload = serde_json::from_str(payload_str)?;
    let status = row
        .get("status")
        .and_then(Value::as_str)
        .and_then(JobStatus::parse_status)
        .ok_or_else(|| JobQueueError::InvalidRow("status".into()))?;
    let attempts = row
        .get("attempts")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0) as u32;
    let run_after = row.get("run_after").and_then(Value::as_i64).unwrap_or(0);
    let last_error = row
        .get("last_error")
        .and_then(Value::as_str)
        .map(str::to_string);
    let dedupe_key = row
        .get("dedupe_key")
        .and_then(Value::as_str)
        .map(str::to_string);
    let created_at = row.get("created_at").and_then(Value::as_i64).unwrap_or(0);
    Ok(Job {
        id: id as u64,
        kind,
        payload,
        status,
        attempts,
        run_after,
        last_error,
        dedupe_key,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_status_roundtrip() {
        for s in [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Succeeded,
            JobStatus::Dead,
        ] {
            assert_eq!(JobStatus::parse_status(s.as_str()), Some(s));
        }
        assert_eq!(JobStatus::parse_status("unknown"), None);
    }

    #[test]
    fn test_backoff_delay_respects_cap() {
        let config = JobQueueConfig {
            max_attempts: 10,
            backoff_base_secs: 1,
            backoff_cap_secs: 64,
            jitter_ratio: 0.0,
            ..JobQueueConfig::default()
        };
        // attempts=10 → exp 封顶 6 → 2^6 = 64s；抖动 0 时精确
        let delay = backoff_delay_ms(&config, 10);
        assert!((delay as f64 - 64_000.0).abs() < 1.0, "delay={delay}");
    }

    #[test]
    fn test_backoff_delay_jitter_range() {
        let config = JobQueueConfig {
            max_attempts: 8,
            backoff_base_secs: 1,
            backoff_cap_secs: 64,
            jitter_ratio: 0.5,
            ..JobQueueConfig::default()
        };
        // attempts=2 → 2^2 = 4s，抖动 0~50% 上浮 → 4~6s
        for _ in 0..50 {
            let delay = backoff_delay_ms(&config, 2);
            assert!(delay >= 4_000, "delay={delay}");
            assert!(delay <= 6_000, "delay={delay}");
        }
    }

    #[test]
    fn test_backoff_delay_escalation() {
        let config = JobQueueConfig {
            max_attempts: 8,
            backoff_base_secs: 1,
            backoff_cap_secs: 64,
            jitter_ratio: 0.0,
            ..JobQueueConfig::default()
        };
        // attempts=3 → 2^3 = 8s
        let delay = backoff_delay_ms(&config, 3);
        assert!((delay as f64 - 8_000.0).abs() < 1.0, "delay={delay}");
    }

    #[test]
    fn test_now_ms_monotonic() {
        let a = now_ms();
        std::thread::sleep(Duration::from_millis(5));
        let b = now_ms();
        assert!(b > a);
    }

    #[test]
    fn test_job_error_kind() {
        let t = JobError::Temporary("downstream 503".into());
        assert_eq!(t.kind(), JobErrorKind::Temporary);
        let p = JobError::Permanent("user not found".into());
        assert_eq!(p.kind(), JobErrorKind::Permanent);
    }

    fn make_row() -> HashMap<String, Value> {
        let mut row = HashMap::new();
        row.insert("id".into(), Value::I64(42));
        row.insert("kind".into(), Value::String("email".into()));
        row.insert("payload".into(), Value::String(r#"{"to":"a@b"}"#.into()));
        row.insert("status".into(), Value::String("pending".into()));
        row.insert("attempts".into(), Value::I64(3));
        row.insert("run_after".into(), Value::I64(1000));
        row.insert("last_error".into(), Value::String("timeout".into()));
        row.insert("dedupe_key".into(), Value::String("k1".into()));
        row.insert("created_at".into(), Value::I64(500));
        row
    }

    #[test]
    fn test_row_to_job_success() {
        let job = row_to_job(make_row()).unwrap();
        assert_eq!(job.id, 42);
        assert_eq!(job.kind, "email");
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 3);
        assert_eq!(job.run_after, 1000);
        assert_eq!(job.last_error.as_deref(), Some("timeout"));
        assert_eq!(job.dedupe_key.as_deref(), Some("k1"));
        assert_eq!(job.created_at, 500);
    }

    #[test]
    fn test_row_to_job_missing_id() {
        let mut row = make_row();
        row.remove("id");
        let err = row_to_job(row).unwrap_err();
        assert!(matches!(err, JobQueueError::InvalidRow(_)));
    }

    #[test]
    fn test_row_to_job_missing_kind() {
        let mut row = make_row();
        row.remove("kind");
        let err = row_to_job(row).unwrap_err();
        assert!(matches!(err, JobQueueError::InvalidRow(_)));
    }

    #[test]
    fn test_row_to_job_missing_payload() {
        let mut row = make_row();
        row.remove("payload");
        let err = row_to_job(row).unwrap_err();
        assert!(matches!(err, JobQueueError::InvalidRow(_)));
    }

    #[test]
    fn test_row_to_job_invalid_status() {
        let mut row = make_row();
        row.insert("status".into(), Value::String("unknown".into()));
        let err = row_to_job(row).unwrap_err();
        assert!(matches!(err, JobQueueError::InvalidRow(_)));
    }

    #[test]
    fn test_row_to_job_invalid_payload_json() {
        let mut row = make_row();
        row.insert("payload".into(), Value::String("{bad json".into()));
        let err = row_to_job(row).unwrap_err();
        assert!(matches!(err, JobQueueError::Json(_)));
    }

    #[test]
    fn test_row_to_job_optional_fields_default() {
        let mut row = make_row();
        row.remove("last_error");
        row.remove("dedupe_key");
        row.remove("attempts");
        row.remove("run_after");
        row.remove("created_at");
        let job = row_to_job(row).unwrap();
        assert_eq!(job.attempts, 0);
        assert_eq!(job.run_after, 0);
        assert!(job.last_error.is_none());
        assert!(job.dedupe_key.is_none());
        assert_eq!(job.created_at, 0);
    }

    #[test]
    fn test_job_queue_config_default() {
        let config = JobQueueConfig::default();
        assert_eq!(config.batch_size, 10);
        assert_eq!(config.max_attempts, 8);
        assert_eq!(config.backoff_base_secs, 1);
        assert_eq!(config.backoff_cap_secs, 64);
        assert_eq!(config.lease_seconds, 60);
    }

    // ---- mock Pool 用于覆盖 JobQueue 构造与访问器 ----

    use std::future::Future;
    use std::pin::Pin;
    use sz_orm_core::{Connection, ConnectionFactory, PoolConfig, QueryRows};

    struct MockConnection;

    impl Connection for MockConnection {
        fn execute<'a>(
            &'a mut self,
            _sql: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
            Box::pin(async { Ok(0) })
        }
        fn query<'a>(
            &'a mut self,
            _sql: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn execute_with_params<'a>(
            &'a mut self,
            _sql: &'a str,
            _params: &'a [Value],
        ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
            Box::pin(async { Ok(1) })
        }
        fn query_with_params<'a>(
            &'a mut self,
            sql: &'a str,
            _params: &'a [Value],
        ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
            Box::pin(async move {
                if sql.contains("LAST_INSERT_ID") {
                    let mut row = HashMap::new();
                    row.insert("id".into(), Value::I64(1));
                    Ok(vec![row])
                } else {
                    Ok(vec![])
                }
            })
        }
        fn begin_transaction<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn commit<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn rollback<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async { true })
        }
        fn close<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    struct MockConnectionFactory;

    #[async_trait]
    impl ConnectionFactory for MockConnectionFactory {
        async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
            Ok(Box::new(MockConnection))
        }
    }

    fn make_mock_pool() -> Arc<Pool> {
        let config = PoolConfig::default();
        let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory);
        Arc::new(Pool::new(config, factory).unwrap())
    }

    #[test]
    fn test_job_queue_new_and_pool() {
        let pool = make_mock_pool();
        let queue = JobQueue::new(pool.clone());
        assert!(Arc::ptr_eq(queue.pool(), &pool));
    }

    #[tokio::test]
    async fn test_job_queue_init_schema() {
        let queue = JobQueue::new(make_mock_pool());
        let result = queue.init_schema().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_job_queue_enqueue() {
        let queue = JobQueue::new(make_mock_pool());
        let id = queue
            .enqueue("email", serde_json::json!({"to": "a@b"}), None)
            .await
            .unwrap();
        assert_eq!(id, 1);
    }

    #[tokio::test]
    async fn test_job_queue_enqueue_with_dedupe() {
        let queue = JobQueue::new(make_mock_pool());
        let id = queue
            .enqueue("email", serde_json::json!({}), Some("k1"))
            .await
            .unwrap();
        assert_eq!(id, 1);
    }

    #[tokio::test]
    async fn test_job_queue_enqueue_delayed() {
        let queue = JobQueue::new(make_mock_pool());
        let id = queue
            .enqueue_delayed(
                "email",
                serde_json::json!({}),
                None,
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(id, 1);
    }

    #[tokio::test]
    async fn test_job_queue_retry_dead() {
        let queue = JobQueue::new(make_mock_pool());
        let result = queue.retry_dead(42).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_job_queue_snapshot() {
        let queue = JobQueue::new(make_mock_pool());
        let snap = queue.queue_snapshot().await.unwrap();
        assert_eq!(snap.pending, 0);
        assert_eq!(snap.running, 0);
    }
}
