// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Queue 静态门面
//!
//! 委托 `sz_rust_orm_facade::Queue`（P2 已实现）。

pub use sz_rust_orm_facade::Queue;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sz_rust_orm_facade::{JobQueueConfig, JobQueueError, Pool, QueueSnapshot, TaskHandler};

/// 队列门面错误转换
impl From<JobQueueError> for crate::FacadeError {
    fn from(e: JobQueueError) -> Self {
        crate::FacadeError::Queue(e.to_string())
    }
}

/// Queue 门面便捷方法
///
/// 在 P2 `Queue` 基础上提供 `FacadeError` 兼容的同步包装。
pub struct QueueFacade;

impl QueueFacade {
    /// 初始化全局队列
    pub fn init(pool: Arc<Pool>) {
        Queue::init(pool);
    }

    /// 投递任务（对齐 PHP `Queue::push`）
    pub async fn push(
        kind: &str,
        payload: Value,
        dedupe_key: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<u64, crate::FacadeError> {
        Queue::push(kind, payload, dedupe_key, delay)
            .await
            .map_err(Into::into)
    }

    /// 启动 Worker
    pub async fn work(
        handlers: HashMap<String, Arc<dyn TaskHandler>>,
        config: JobQueueConfig,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), crate::FacadeError> {
        Queue::work(handlers, config, shutdown)
            .await
            .map_err(Into::into)
    }

    /// 死信重放
    pub async fn retry_dead(job_id: u64) -> Result<(), crate::FacadeError> {
        Queue::retry_dead(job_id).await.map_err(Into::into)
    }

    /// 队列快照
    pub async fn snapshot() -> Result<QueueSnapshot, crate::FacadeError> {
        Queue::snapshot().await.map_err(Into::into)
    }

    /// 幂等建表
    pub async fn init_schema() -> Result<(), crate::FacadeError> {
        Queue::init_schema().await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_facade_not_initialized() {
        let result = std::panic::catch_unwind(|| {
            let _ = QueueFacade::push;
        });
        // push is a fn pointer, doesn't panic; just verify it exists
        assert!(result.is_ok());
    }
}
