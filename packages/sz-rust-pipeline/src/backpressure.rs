// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 背压控制（spec §5.8 规则 3、§6.8 规则 3）
//!
//! 下游过载时上游限速而非丢数据。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::error::PipelineError;

/// 背压配置
#[derive(Debug, Clone)]
pub struct BackpressureConfig {
    /// 队列上限
    pub threshold: usize,
    /// 限速等待超时
    pub timeout: Duration,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            threshold: 1024,
            timeout: Duration::from_secs(5),
        }
    }
}

/// 背压控制器
pub struct BackpressureController {
    config: BackpressureConfig,
    /// 当前队列深度
    depth: AtomicUsize,
    /// 并发许可
    semaphore: Semaphore,
    /// 背压触发次数
    triggers: AtomicUsize,
}

impl BackpressureController {
    /// 创建背压控制器
    pub fn new(config: BackpressureConfig) -> Self {
        let permits = config.threshold.max(1);
        Self {
            config,
            depth: AtomicUsize::new(0),
            semaphore: Semaphore::new(permits),
            triggers: AtomicUsize::new(0),
        }
    }

    /// 获取许可（背压触发时限速等待）
    pub async fn acquire(&self) -> Result<BackpressurePermit<'_>, PipelineError> {
        let depth = self.depth.fetch_add(1, Ordering::SeqCst);
        if depth >= self.config.threshold {
            self.triggers.fetch_add(1, Ordering::SeqCst);
        }

        let permit = tokio::time::timeout(self.config.timeout, self.semaphore.acquire())
            .await
            .map_err(|_| PipelineError::BackpressureSustained)?
            .map_err(|_| PipelineError::BackpressureSustained)?;

        Ok(BackpressurePermit {
            permit,
            depth: &self.depth,
        })
    }

    /// 当前队列深度
    pub fn current_depth(&self) -> usize {
        self.depth.load(Ordering::SeqCst)
    }

    /// 背压触发次数
    pub fn trigger_count(&self) -> usize {
        self.triggers.load(Ordering::SeqCst)
    }

    /// 阈值
    pub fn threshold(&self) -> usize {
        self.config.threshold
    }
}

/// 背压许可（RAII guard）
pub struct BackpressurePermit<'a> {
    /// 并发许可（drop 时自动释放）
    #[allow(dead_code)]
    permit: tokio::sync::SemaphorePermit<'a>,
    /// 队列深度计数器引用
    depth: &'a AtomicUsize,
}

impl<'a> Drop for BackpressurePermit<'a> {
    fn drop(&mut self) {
        self.depth.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_backpressure_acquire_release() {
        let controller = BackpressureController::new(BackpressureConfig {
            threshold: 10,
            timeout: Duration::from_secs(1),
        });

        {
            let _permit = controller.acquire().await.unwrap();
            assert_eq!(controller.current_depth(), 1);
        }
        assert_eq!(controller.current_depth(), 0);
    }

    #[tokio::test]
    async fn test_backpressure_no_trigger_under_threshold() {
        let controller = BackpressureController::new(BackpressureConfig {
            threshold: 10,
            timeout: Duration::from_secs(1),
        });

        let _permit = controller.acquire().await.unwrap();
        assert_eq!(controller.trigger_count(), 0);
    }

    #[test]
    fn test_backpressure_config_default() {
        let config = BackpressureConfig::default();
        assert_eq!(config.threshold, 1024);
        assert_eq!(config.timeout, Duration::from_secs(5));
    }
}
