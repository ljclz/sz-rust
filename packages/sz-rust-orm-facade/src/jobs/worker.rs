// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 优先级队列 Worker（T008）
//!
//! 按队列优先级消费（高优先级队列优先），支持 SIGTERM/SIGINT 优雅停机。
//! 收到信号后停止领取新 Job、等待当前 Job 完成、退出。

use std::collections::HashMap;
use std::sync::Arc;

use super::{Job, JobError, JobQueueConfig, JobQueueError, QueueBackend, TaskHandler};

/// 队列条目（名称 + 优先级 + 后端）
struct QueueEntry {
    name: String,
    priority: i32,
    backend: Arc<dyn QueueBackend>,
}

/// 优先级队列 Worker（T008）
///
/// 按队列优先级消费：高优先级队列优先轮询，低优先级队列在高优先级空时消费。
/// 支持 SIGTERM/SIGINT 优雅停机：收到信号后停止领取新 Job、等待当前 Job 完成。
pub struct PriorityQueueWorker {
    queues: Vec<QueueEntry>,
    handlers: HashMap<String, Arc<dyn TaskHandler>>,
    config: JobQueueConfig,
}

impl PriorityQueueWorker {
    /// 创建优先级 Worker
    pub fn new() -> Self {
        Self {
            queues: Vec::new(),
            handlers: HashMap::new(),
            config: JobQueueConfig::default(),
        }
    }

    /// 添加队列（优先级降序消费，高优先级先）
    pub fn with_queue(
        mut self,
        name: impl Into<String>,
        priority: i32,
        backend: Arc<dyn QueueBackend>,
    ) -> Self {
        self.queues.push(QueueEntry {
            name: name.into(),
            priority,
            backend,
        });
        self
    }

    /// 注册任务处理器
    pub fn with_handler(mut self, kind: impl Into<String>, handler: Arc<dyn TaskHandler>) -> Self {
        self.handlers.insert(kind.into(), handler);
        self
    }

    /// 设置 worker 配置
    pub fn with_config(mut self, config: JobQueueConfig) -> Self {
        self.config = config;
        self
    }

    /// 启动 Worker，监听 SIGTERM/SIGINT 优雅停机
    ///
    /// 收到信号后停止领取新 Job，等待当前 Job 完成后退出。
    pub async fn run(mut self) -> Result<(), JobQueueError> {
        self.queues.sort_by_key(|e| std::cmp::Reverse(e.priority));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let signal_handle = tokio::spawn(wait_for_shutdown_signal(shutdown_tx));

        let mut interval = tokio::time::interval(self.config.poll_interval);
        loop {
            interval.tick().await;

            if *shutdown_rx.borrow() {
                tracing::info!(target: "sz_orm::jobs::worker", "priority worker shutting down");
                signal_handle.abort();
                return Ok(());
            }

            let mut any_jobs = false;
            for entry in &self.queues {
                if *shutdown_rx.borrow() {
                    break;
                }

                if let Err(e) = entry.backend.reclaim_stale(&self.config).await {
                    tracing::error!(
                        target: "sz_orm::jobs::worker",
                        "reclaim stale failed for queue '{}': {e}",
                        entry.name
                    );
                    continue;
                }

                let jobs = match entry.backend.pop(&self.config).await {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!(
                            target: "sz_orm::jobs::worker",
                            "pop failed for queue '{}': {e}",
                            entry.name
                        );
                        continue;
                    }
                };

                if jobs.is_empty() {
                    continue;
                }

                any_jobs = true;
                for job in jobs {
                    if *shutdown_rx.borrow() {
                        tracing::info!(
                            target: "sz_orm::jobs::worker",
                            "shutdown signaled, skipping remaining jobs"
                        );
                        break;
                    }
                    self.process_job(&entry.backend, job).await?;
                }
            }

            if !any_jobs {
                tokio::time::sleep(self.config.poll_interval).await;
            }
        }
    }

    /// 处理单个 Job
    async fn process_job(
        &self,
        backend: &Arc<dyn QueueBackend>,
        job: Job,
    ) -> Result<(), JobQueueError> {
        let handler = self.handlers.get(&job.kind);
        let outcome = match handler {
            Some(h) => {
                match tokio::time::timeout(self.config.handler_timeout, h.handle(&job.payload))
                    .await
                {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(JobError::Temporary(format!(
                        "handler timeout after {:?}",
                        self.config.handler_timeout
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
                backend.ack(job.id).await?;
                tracing::debug!(
                    target: "sz_orm::jobs::worker",
                    "job {} (kind={}) succeeded",
                    job.id,
                    job.kind
                );
            }
            Err(e) => {
                backend
                    .fail(job.id, job.attempts, &e.to_string(), e.kind(), &self.config)
                    .await?;
                tracing::warn!(
                    target: "sz_orm::jobs::worker",
                    "job {} (kind={}) failed: {} (kind={:?}), attempts={}",
                    job.id,
                    job.kind,
                    e,
                    e.kind(),
                    job.attempts
                );
            }
        }
        Ok(())
    }
}

impl Default for PriorityQueueWorker {
    fn default() -> Self {
        Self::new()
    }
}

/// 等待停机信号（SIGTERM/SIGINT），收到后发送 shutdown 通知
async fn wait_for_shutdown_signal(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(target: "sz_orm::jobs::worker", "failed to install SIGTERM handler: {e}");
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(target: "sz_orm::jobs::worker", "received SIGINT, initiating shutdown");
            }
            _ = sigterm.recv() => {
                tracing::info!(target: "sz_orm::jobs::worker", "received SIGTERM, initiating shutdown");
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(target: "sz_orm::jobs::worker", "failed to install Ctrl-C handler: {e}");
            return;
        }
        tracing::info!(target: "sz_orm::jobs::worker", "received Ctrl-C, initiating shutdown");
    }
    let _ = shutdown_tx.send(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct EchoHandler;
    #[async_trait]
    impl TaskHandler for EchoHandler {
        async fn handle(&self, _payload: &serde_json::Value) -> Result<(), JobError> {
            Ok(())
        }
    }

    #[test]
    fn test_priority_worker_builder() {
        let worker = PriorityQueueWorker::new()
            .with_config(JobQueueConfig::default())
            .with_handler("echo", Arc::new(EchoHandler));
        assert_eq!(worker.handlers.len(), 1);
        assert!(worker.queues.is_empty());
    }

    #[test]
    fn test_priority_worker_default() {
        let worker = PriorityQueueWorker::default();
        assert!(worker.queues.is_empty());
        assert!(worker.handlers.is_empty());
    }
}
