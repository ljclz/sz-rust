// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Saga 编排器（T024）
//!
//! 正向依次执行 + 失败逆序补偿 + 超时处理 + 补偿重试

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::warn;

use crate::error::DtxError;

/// Saga 动作 trait
#[async_trait]
pub trait SagaAction: Send + Sync + 'static {
    /// 执行动作
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError>;
}

/// Saga 步骤
pub struct SagaStep {
    /// 步骤 ID
    pub step_id: String,
    /// 正向操作
    pub forward: Arc<dyn SagaAction>,
    /// 补偿操作
    pub compensate: Arc<dyn SagaAction>,
}

impl SagaStep {
    /// 构造 Saga 步骤
    pub fn new(
        step_id: impl Into<String>,
        forward: Arc<dyn SagaAction>,
        compensate: Arc<dyn SagaAction>,
    ) -> Self {
        Self {
            step_id: step_id.into(),
            forward,
            compensate,
        }
    }
}

/// 步骤执行状态
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// 待执行
    Pending,
    /// 正向执行成功
    ForwardDone,
    /// 正向执行失败
    ForwardFailed,
    /// 补偿执行成功
    Compensated,
    /// 补偿失败（待人工处理）
    CompensateFailed,
    /// 超时
    Timeout,
}

/// 单步执行结果
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub status: StepStatus,
    pub output: Option<Value>,
    pub error: Option<String>,
}

/// Saga 执行结果
#[derive(Debug, Clone)]
pub struct SagaResult {
    /// 事务 ID
    pub tx_id: String,
    /// 是否全部成功
    pub success: bool,
    /// 各步骤结果
    pub steps: Vec<StepResult>,
    /// 最终 payload
    pub final_payload: Value,
    /// 失败的步骤 ID（如有）
    pub failed_step: Option<String>,
}

/// Saga 编排器
pub struct SagaOrchestrator {
    /// 补偿重试次数
    compensate_retries: u32,
    /// 补偿重试间隔
    retry_interval: Duration,
    /// 超时时间
    timeout: Option<Duration>,
}

impl Default for SagaOrchestrator {
    fn default() -> Self {
        Self {
            compensate_retries: 3,
            retry_interval: Duration::from_millis(100),
            timeout: None,
        }
    }
}

impl SagaOrchestrator {
    /// 创建 Saga 编排器
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置补偿重试次数
    pub fn with_compensate_retries(mut self, retries: u32) -> Self {
        self.compensate_retries = retries.max(1);
        self
    }

    /// 设置重试间隔
    pub fn with_retry_interval(mut self, interval: Duration) -> Self {
        self.retry_interval = interval;
        self
    }

    /// 设置超时
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// 执行 Saga 事务
    pub async fn execute(
        &self,
        steps: Vec<SagaStep>,
        payload: Value,
    ) -> Result<SagaResult, DtxError> {
        let tx_id = format!("saga-{}", chrono::Utc::now().timestamp_millis());
        let mut current_payload = payload;
        let mut step_results: Vec<StepResult> = Vec::with_capacity(steps.len());
        let mut completed_indices: Vec<usize> = Vec::new();

        let forward_result = self
            .execute_forward(
                &steps,
                &mut current_payload,
                &mut step_results,
                &mut completed_indices,
            )
            .await;

        match forward_result {
            Ok(()) => Ok(SagaResult {
                tx_id,
                success: true,
                steps: step_results,
                final_payload: current_payload,
                failed_step: None,
            }),
            Err(failed_step) => {
                let compensate_result = self
                    .execute_compensate(
                        &steps,
                        &completed_indices,
                        &mut step_results,
                        &mut current_payload,
                    )
                    .await;

                if let Err(e) = compensate_result {
                    warn!(tx_id = %tx_id, error = %e, "compensation partially failed");
                }

                Ok(SagaResult {
                    tx_id,
                    success: false,
                    steps: step_results,
                    final_payload: current_payload,
                    failed_step: Some(failed_step),
                })
            }
        }
    }

    async fn execute_forward(
        &self,
        steps: &[SagaStep],
        payload: &mut Value,
        results: &mut Vec<StepResult>,
        completed: &mut Vec<usize>,
    ) -> Result<(), String> {
        for (idx, step) in steps.iter().enumerate() {
            let forward = self.run_with_timeout(step.forward.as_ref(), payload);

            match forward.await {
                Ok(output) => {
                    *payload = output.clone();
                    results.push(StepResult {
                        step_id: step.step_id.clone(),
                        status: StepStatus::ForwardDone,
                        output: Some(output),
                        error: None,
                    });
                    completed.push(idx);
                }
                Err(e) => {
                    results.push(StepResult {
                        step_id: step.step_id.clone(),
                        status: StepStatus::ForwardFailed,
                        output: None,
                        error: Some(e.to_string()),
                    });
                    return Err(step.step_id.clone());
                }
            }
        }
        Ok(())
    }

    async fn execute_compensate(
        &self,
        steps: &[SagaStep],
        completed: &[usize],
        results: &mut Vec<StepResult>,
        payload: &mut Value,
    ) -> Result<(), DtxError> {
        for &idx in completed.iter().rev() {
            let step = &steps[idx];
            let mut last_err: Option<DtxError> = None;

            for attempt in 0..self.compensate_retries {
                if attempt > 0 {
                    tokio::time::sleep(self.retry_interval).await;
                }

                match step.compensate.execute(payload).await {
                    Ok(output) => {
                        *payload = output.clone();
                        results.push(StepResult {
                            step_id: step.step_id.clone(),
                            status: StepStatus::Compensated,
                            output: Some(output),
                            error: None,
                        });
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        warn!(
                            step_id = %step.step_id,
                            attempt = attempt + 1,
                            error = %e,
                            "compensate attempt failed"
                        );
                        last_err = Some(e);
                    }
                }
            }

            if let Some(e) = last_err {
                results.push(StepResult {
                    step_id: step.step_id.clone(),
                    status: StepStatus::CompensateFailed,
                    output: None,
                    error: Some(e.to_string()),
                });
                return Err(DtxError::CompensateRetryExhausted {
                    step_id: step.step_id.clone(),
                    retries: self.compensate_retries,
                });
            }
        }
        Ok(())
    }

    async fn run_with_timeout(
        &self,
        action: &dyn SagaAction,
        payload: &Value,
    ) -> Result<Value, DtxError> {
        match self.timeout {
            Some(d) => tokio::time::timeout(d, action.execute(payload))
                .await
                .map_err(|_| DtxError::Timeout(d))?,
            None => action.execute(payload).await,
        }
    }
}

/// 闭包包装为 SagaAction
pub struct FnAction<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, DtxError>> + Send + 'static,
{
    func: F,
}

impl<F, Fut> FnAction<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, DtxError>> + Send + 'static,
{
    /// 创建闭包动作
    pub fn new(func: F) -> Self {
        Self { func }
    }
}

#[async_trait]
impl<F, Fut> SagaAction for FnAction<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, DtxError>> + Send + 'static,
{
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        (self.func)(payload.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_action(counter: Arc<AtomicUsize>, should_fail: bool) -> Arc<dyn SagaAction> {
        Arc::new(FnAction::new(move |payload: Value| {
            let c = counter.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                if should_fail {
                    return Err(DtxError::generic("action failed"));
                }
                Ok(payload)
            }
        }))
    }

    #[tokio::test]
    async fn test_saga_all_success() {
        let forward_calls = Arc::new(AtomicUsize::new(0));
        let compensate_calls = Arc::new(AtomicUsize::new(0));

        let steps = vec![SagaStep::new(
            "step-1",
            make_action(forward_calls.clone(), false),
            make_action(compensate_calls.clone(), false),
        )];

        let orchestrator = SagaOrchestrator::new();
        let result = orchestrator.execute(steps, Value::Null).await.unwrap();

        assert!(result.success);
        assert_eq!(result.failed_step, None);
        assert_eq!(forward_calls.load(Ordering::SeqCst), 1);
        assert_eq!(compensate_calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].status, StepStatus::ForwardDone);
    }

    #[tokio::test]
    async fn test_saga_forward_fail_triggers_compensate() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let f2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "step-1",
                make_action(f1.clone(), false),
                make_action(c1.clone(), false),
            ),
            SagaStep::new(
                "step-2",
                make_action(f2.clone(), true),
                make_action(c2.clone(), false),
            ),
        ];

        let orchestrator = SagaOrchestrator::new();
        let result = orchestrator.execute(steps, Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_step.as_deref(), Some("step-2"));
        assert_eq!(f1.load(Ordering::SeqCst), 1);
        assert_eq!(f2.load(Ordering::SeqCst), 1);
        assert_eq!(c1.load(Ordering::SeqCst), 1, "step-1 compensate should run");
        assert_eq!(
            c2.load(Ordering::SeqCst),
            0,
            "step-2 compensate should NOT run (forward failed)"
        );
    }

    #[tokio::test]
    async fn test_saga_compensate_retry() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "step-1",
                make_action(f1.clone(), false),
                make_action(c1.clone(), false),
            ),
            SagaStep::new(
                "step-2",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let orchestrator = SagaOrchestrator::new()
            .with_compensate_retries(3)
            .with_retry_interval(Duration::from_millis(1));
        let result = orchestrator.execute(steps, Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(
            c1.load(Ordering::SeqCst),
            1,
            "compensate should succeed on first try"
        );
    }

    #[tokio::test]
    async fn test_saga_compensate_fail_marked() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "step-1",
                make_action(f1.clone(), false),
                make_action(c1.clone(), true),
            ),
            SagaStep::new(
                "step-2",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let orchestrator = SagaOrchestrator::new()
            .with_compensate_retries(2)
            .with_retry_interval(Duration::from_millis(1));
        let result = orchestrator.execute(steps, Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(
            c1.load(Ordering::SeqCst),
            2,
            "compensate should retry 2 times"
        );

        let failed_compensate = result
            .steps
            .iter()
            .find(|s| s.status == StepStatus::CompensateFailed);
        assert!(
            failed_compensate.is_some(),
            "should have a CompensateFailed step"
        );
    }

    #[tokio::test]
    async fn test_saga_empty_steps() {
        let orchestrator = SagaOrchestrator::new();
        let result = orchestrator.execute(vec![], Value::Null).await.unwrap();
        assert!(result.success);
        assert!(result.steps.is_empty());
    }

    #[tokio::test]
    async fn test_saga_timeout() {
        let slow_action = Arc::new(FnAction::new(|_payload: Value| async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(Value::Null)
        }));
        let noop = Arc::new(FnAction::new(|p: Value| async move { Ok(p) }));

        let steps = vec![SagaStep::new("slow", slow_action, noop)];
        let orchestrator = SagaOrchestrator::new().with_timeout(Duration::from_millis(50));
        let result = orchestrator.execute(steps, Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_step.as_deref(), Some("slow"));
    }

    #[test]
    fn test_step_status_serde() {
        let s = serde_json::to_string(&StepStatus::ForwardDone).unwrap();
        assert_eq!(s, "\"forward_done\"");
        let v: StepStatus = serde_json::from_str("\"compensate_failed\"").unwrap();
        assert_eq!(v, StepStatus::CompensateFailed);
    }

    #[tokio::test]
    async fn test_saga_three_steps_fail_at_second() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let f3 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "s1",
                make_action(f1.clone(), false),
                make_action(c1.clone(), false),
            ),
            SagaStep::new(
                "s2",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            SagaStep::new(
                "s3",
                make_action(f3.clone(), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let orchestrator = SagaOrchestrator::new();
        let result = orchestrator.execute(steps, Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_step.as_deref(), Some("s2"));
        assert_eq!(f1.load(Ordering::SeqCst), 1, "s1 forward ran");
        assert_eq!(f3.load(Ordering::SeqCst), 0, "s3 forward should NOT run");
        assert_eq!(
            c1.load(Ordering::SeqCst),
            1,
            "s1 compensate ran (reverse order)"
        );
    }
}
