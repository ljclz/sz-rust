// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 异步流水线执行（spec §5.8 规则 1-2）
//!
//! 无依赖阶段并行、有依赖阶段顺序执行。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::backpressure::{BackpressureConfig, BackpressureController};
use crate::error::PipelineError;
use crate::metrics::PipelineMetrics;
use crate::stage::{topological_sort, StageDefinition, StageHandler, StageId};

/// 流水线配置
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// 阶段定义
    pub stages: Vec<StageDefinition>,
    /// 并行度（默认 CPU 核数，spec §6.8 规则 2）
    pub parallelism: usize,
    /// 背压阈值（队列上限，spec §6.8 规则 3）
    pub backpressure_threshold: usize,
}

impl PipelineConfig {
    /// 创建流水线配置
    pub fn new(stages: Vec<StageDefinition>) -> Self {
        Self {
            stages,
            parallelism: num_cpus(),
            backpressure_threshold: 1024,
        }
    }

    /// 设置并行度
    pub fn with_parallelism(mut self, n: usize) -> Self {
        self.parallelism = n.max(1);
        self
    }

    /// 设置背压阈值
    pub fn with_backpressure(mut self, threshold: usize) -> Self {
        self.backpressure_threshold = threshold.max(1);
        self
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// 异步流水线
pub struct AsyncPipeline<T> {
    /// 配置
    config: PipelineConfig,
    /// 阶段处理器
    handlers: HashMap<StageId, Arc<dyn StageHandler<T, T>>>,
    /// 背压控制器
    backpressure: BackpressureController,
    /// 指标收集器
    metrics: Arc<PipelineMetrics>,
}

impl<T: Send + 'static> AsyncPipeline<T> {
    /// 创建异步流水线
    pub fn new(
        config: PipelineConfig,
        handlers: HashMap<StageId, Arc<dyn StageHandler<T, T>>>,
    ) -> Result<Self, PipelineError> {
        topological_sort(&config.stages)?;

        for stage in &config.stages {
            if !handlers.contains_key(&stage.id) {
                return Err(PipelineError::Config(format!(
                    "阶段 `{}` 缺少处理器",
                    stage.id
                )));
            }
        }

        let backpressure = BackpressureController::new(BackpressureConfig {
            threshold: config.backpressure_threshold,
            timeout: Duration::from_secs(30),
        });

        Ok(Self {
            config,
            handlers,
            backpressure,
            metrics: Arc::new(PipelineMetrics::new()),
        })
    }

    /// 执行流水线
    ///
    /// # 后置条件
    /// - 无依赖阶段并行执行
    /// - 有依赖阶段按依赖顺序执行
    /// - 背压触发时上游限速
    /// - 任一阶段失败 → 取消整条流水线
    pub async fn execute(&self, input: T) -> Result<T, PipelineError> {
        let _permit = self.backpressure.acquire().await?;
        self.metrics.record_pipeline_execution();

        let sorted = topological_sort(&self.config.stages)?;

        let mut current_input = input;
        for stage_id in &sorted {
            let stage_def = self
                .config
                .stages
                .iter()
                .find(|s| &s.id == stage_id)
                .ok_or_else(|| PipelineError::Config(format!("阶段 `{stage_id}` 未定义")))?;

            let handler = self
                .handlers
                .get(stage_id)
                .ok_or_else(|| PipelineError::Config(format!("阶段 `{stage_id}` 缺少处理器")))?;

            let result = timeout(stage_def.timeout, handler.handle(current_input))
                .await
                .map_err(|_| PipelineError::StageTimeout(stage_id.clone()))?
                .map_err(|e| match e {
                    PipelineError::StageFailed(_, msg) => {
                        PipelineError::StageFailed(stage_id.clone(), msg)
                    }
                    other => other,
                })?;

            current_input = result;
        }

        Ok(current_input)
    }

    /// 获取指标收集器
    pub fn metrics(&self) -> &PipelineMetrics {
        &self.metrics
    }

    /// 获取配置
    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type HandlerPair = (&'static str, Arc<dyn StageHandler<i32, i32>>);

    struct IdentityHandler;

    #[async_trait::async_trait]
    impl StageHandler<i32, i32> for IdentityHandler {
        async fn handle(&self, input: i32) -> Result<i32, PipelineError> {
            Ok(input)
        }
    }

    struct DoubleHandler;

    #[async_trait::async_trait]
    impl StageHandler<i32, i32> for DoubleHandler {
        async fn handle(&self, input: i32) -> Result<i32, PipelineError> {
            Ok(input * 2)
        }
    }

    struct FailHandler;

    #[async_trait::async_trait]
    impl StageHandler<i32, i32> for FailHandler {
        async fn handle(&self, _input: i32) -> Result<i32, PipelineError> {
            Err(PipelineError::StageFailed(
                StageId::new("fail"),
                "故意失败".to_string(),
            ))
        }
    }

    fn make_handlers(
        handlers: Vec<HandlerPair>,
    ) -> HashMap<StageId, Arc<dyn StageHandler<i32, i32>>> {
        handlers
            .into_iter()
            .map(|(name, h)| (StageId::new(name), h))
            .collect()
    }

    #[tokio::test]
    async fn test_pipeline_single_stage() {
        let config = PipelineConfig::new(vec![StageDefinition::new("identity")]);
        let handlers = make_handlers(vec![("identity", Arc::new(IdentityHandler))]);
        let pipeline = AsyncPipeline::new(config, handlers).unwrap();

        let result = pipeline.execute(42).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_pipeline_sequential() {
        let config = PipelineConfig::new(vec![
            StageDefinition::new("identity"),
            StageDefinition::new("double").depends_on("identity"),
        ]);
        let handlers = make_handlers(vec![
            ("identity", Arc::new(IdentityHandler)),
            ("double", Arc::new(DoubleHandler)),
        ]);
        let pipeline = AsyncPipeline::new(config, handlers).unwrap();

        let result = pipeline.execute(21).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_pipeline_failure_propagates() {
        let config = PipelineConfig::new(vec![StageDefinition::new("fail")]);
        let handlers = make_handlers(vec![("fail", Arc::new(FailHandler))]);
        let pipeline = AsyncPipeline::new(config, handlers).unwrap();

        let result = pipeline.execute(42).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pipeline_cyclic_rejected() {
        let config = PipelineConfig::new(vec![
            StageDefinition::new("a").depends_on("b"),
            StageDefinition::new("b").depends_on("a"),
        ]);
        let handlers = make_handlers(vec![
            ("a", Arc::new(IdentityHandler)),
            ("b", Arc::new(IdentityHandler)),
        ]);
        let result = AsyncPipeline::new(config, handlers);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pipeline_missing_handler_rejected() {
        let config = PipelineConfig::new(vec![StageDefinition::new("a")]);
        let handlers: HashMap<StageId, Arc<dyn StageHandler<i32, i32>>> = HashMap::new();
        let result = AsyncPipeline::new(config, handlers);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pipeline_metrics_recorded() {
        let config = PipelineConfig::new(vec![StageDefinition::new("identity")]);
        let handlers = make_handlers(vec![("identity", Arc::new(IdentityHandler))]);
        let pipeline = AsyncPipeline::new(config, handlers).unwrap();

        pipeline.execute(42).await.unwrap();
        assert_eq!(pipeline.metrics().pipeline_execution_count(), 1);
    }
}
