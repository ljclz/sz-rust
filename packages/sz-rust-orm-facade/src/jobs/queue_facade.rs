// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Queue 静态门面（T009）
//!
//! 对齐 ThinkPHP `think\facade\Queue` 静态外观模式。
//! 委托全局 `OnceCell<JobQueue>` 单例，零开销转发。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_orm_facade::queue_facade::Queue;
//!
//! Queue::init(pool);
//! Queue::push("SendEmail", json!({"to": "user@example.com"}), None, None).await.unwrap();
//! ```

use std::sync::{Arc, OnceLock};

use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use sz_orm_core::Pool;

use super::{JobQueue, JobQueueConfig, JobQueueError, QueueSnapshot, TaskHandler};

/// 全局 JobQueue 单例
static GLOBAL_QUEUE: OnceLock<JobQueue> = OnceLock::new();

/// Queue 静态门面（T009）
///
/// 对齐 PHP `think\facade\Queue`，通过静态方法委托全局 `JobQueue` 单例。
pub struct Queue;

impl Queue {
    /// 初始化全局队列（设置连接池）
    ///
    /// 必须在调用其他方法前调用。重复调用不会覆盖已有实例。
    pub fn init(pool: Arc<Pool>) {
        let _ = GLOBAL_QUEUE.set(JobQueue::new(pool));
    }

    /// 获取全局队列引用（未初始化时 panic）
    fn queue() -> &'static JobQueue {
        GLOBAL_QUEUE
            .get()
            .expect("Queue not initialized, call Queue::init(pool) first")
    }

    /// 投递任务（对齐 PHP `Queue::push($job, $queue = null, $delay = 0)`）
    ///
    /// `delay` 为可选延迟时长，`None` 表示立即执行。
    pub async fn push(
        kind: &str,
        payload: Value,
        dedupe_key: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<u64, JobQueueError> {
        let queue = Self::queue();
        match delay {
            Some(d) => queue.enqueue_delayed(kind, payload, dedupe_key, d).await,
            None => queue.enqueue(kind, payload, dedupe_key).await,
        }
    }

    /// 启动 Worker（对齐 PHP `Queue::work($queue = null)`）
    pub async fn work(
        handlers: HashMap<String, Arc<dyn TaskHandler>>,
        config: JobQueueConfig,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), JobQueueError> {
        Self::queue().run_worker(handlers, config, shutdown).await
    }

    /// 死信重放
    pub async fn retry_dead(job_id: u64) -> Result<(), JobQueueError> {
        Self::queue().retry_dead(job_id).await
    }

    /// 队列快照
    pub async fn snapshot() -> Result<QueueSnapshot, JobQueueError> {
        Self::queue().queue_snapshot().await
    }

    /// 幂等建表
    pub async fn init_schema() -> Result<(), JobQueueError> {
        Self::queue().init_schema().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_not_initialized_panics() {
        let result = std::panic::catch_unwind(|| {
            let _ = Queue::queue();
        });
        assert!(result.is_err());
    }
}
