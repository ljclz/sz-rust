// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 并行 Saga 编排器（P3-1）
//!
//! 无依赖参与者并行执行 + 补偿幂等 + 指数退避重试

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::future::join_all;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tracing::warn;

use crate::error::DtxError;
use crate::saga::{SagaOrchestrator, SagaResult, SagaStep, StepResult, StepStatus};

/// 参与者依赖图
///
/// 描述 Saga 参与者之间的依赖关系，用于拓扑排序识别可并行执行的分组。
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    /// step_id -> 它所依赖的 step_id 列表
    dependencies: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    /// 创建空依赖图
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加依赖关系：`step_id` 依赖于 `depends_on`
    pub fn add_dependency(&mut self, step_id: impl Into<String>, depends_on: impl Into<String>) {
        self.dependencies
            .entry(step_id.into())
            .or_default()
            .push(depends_on.into());
    }

    /// 批量设置依赖
    pub fn with_dependencies(mut self, deps: HashMap<String, Vec<String>>) -> Self {
        self.dependencies = deps;
        self
    }

    /// 拓扑排序：返回按依赖层级分组的步骤 ID
    ///
    /// 每组内的步骤无相互依赖，可并行执行。
    /// 检测到环时返回 `DtxError::InvalidState`。
    pub fn topological_groups(&self, all_steps: &[String]) -> Result<Vec<Vec<String>>, DtxError> {
        let mut remaining: HashSet<String> = all_steps.iter().cloned().collect();
        let mut groups: Vec<Vec<String>> = Vec::new();

        while !remaining.is_empty() {
            let ready: Vec<String> = remaining
                .iter()
                .filter(|step_id| {
                    self.dependencies
                        .get(*step_id)
                        .map(|deps| deps.iter().all(|d| !remaining.contains(d)))
                        .unwrap_or(true)
                })
                .cloned()
                .collect();

            if ready.is_empty() {
                return Err(DtxError::InvalidState(format!(
                    "dependency cycle detected among: {:?}",
                    remaining
                )));
            }

            for s in &ready {
                remaining.remove(s);
            }
            groups.push(ready);
        }

        Ok(groups)
    }
}

/// 指数退避重试策略
#[derive(Debug, Clone)]
pub struct BackoffRetry {
    /// 最大重试次数
    pub max_retries: u32,
    /// 初始重试间隔
    pub initial_interval: Duration,
    /// 退避乘数（≥1）
    pub multiplier: f32,
}

impl Default for BackoffRetry {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_interval: Duration::from_millis(50),
            multiplier: 2.0,
        }
    }
}

impl BackoffRetry {
    /// 创建退避策略
    pub fn new(max_retries: u32, initial_interval: Duration, multiplier: f32) -> Self {
        Self {
            max_retries: max_retries.max(1),
            initial_interval,
            multiplier: multiplier.max(1.0),
        }
    }

    /// 计算第 `attempt` 次重试的间隔（attempt 从 0 开始）
    pub fn interval(&self, attempt: u32) -> Duration {
        let ms = self.initial_interval.as_millis() as f64;
        let factor = self.multiplier.powi(attempt as i32) as f64;
        Duration::from_millis((ms * factor) as u64)
    }
}

/// 补偿幂等包装器
///
/// 记录已补偿的步骤，确保重复补偿无副作用。
#[derive(Debug, Default)]
pub struct IdempotentCompensate {
    compensated: Mutex<HashSet<String>>,
}

impl IdempotentCompensate {
    /// 创建幂等补偿器
    pub fn new() -> Self {
        Self::default()
    }

    /// 标记步骤已补偿，返回是否为首次补偿
    ///
    /// 若步骤已补偿过，返回 false（幂等跳过）。
    pub fn mark(&self, step_id: &str) -> bool {
        self.compensated.lock().insert(step_id.to_string())
    }

    /// 检查步骤是否已补偿
    pub fn is_compensated(&self, step_id: &str) -> bool {
        self.compensated.lock().contains(step_id)
    }

    /// 已补偿步骤数
    pub fn count(&self) -> usize {
        self.compensated.lock().len()
    }
}

/// 并行 Saga 编排器
///
/// 基于依赖图将无依赖的参与者分组并行执行，
/// 失败时按执行逆序补偿，补偿幂等且支持指数退避重试。
pub struct ParallelSaga {
    orchestrator: SagaOrchestrator,
    dependency_graph: DependencyGraph,
    backoff: BackoffRetry,
}

impl ParallelSaga {
    /// 创建并行 Saga 编排器
    pub fn new(orchestrator: SagaOrchestrator) -> Self {
        Self {
            orchestrator,
            dependency_graph: DependencyGraph::new(),
            backoff: BackoffRetry::default(),
        }
    }

    /// 设置依赖图
    pub fn with_dependency_graph(mut self, graph: DependencyGraph) -> Self {
        self.dependency_graph = graph;
        self
    }

    /// 设置退避重试策略
    pub fn with_backoff(mut self, retry: BackoffRetry) -> Self {
        self.backoff = retry;
        self
    }

    /// 执行并行 Saga 事务
    ///
    /// 按依赖图拓扑排序分组，组内并行执行，组间顺序执行。
    /// 任一参与者失败时，对已完成参与者按逆序补偿。
    pub async fn execute(
        &self,
        steps: &[SagaStep],
        payload: &Value,
    ) -> Result<SagaResult, DtxError> {
        let tx_id = format!("psaga-{}", chrono::Utc::now().timestamp_millis());
        let all_step_ids: Vec<String> = steps.iter().map(|s| s.step_id.clone()).collect();
        let step_map: HashMap<String, &SagaStep> =
            steps.iter().map(|s| (s.step_id.clone(), s)).collect();

        let groups = self.dependency_graph.topological_groups(&all_step_ids)?;

        let mut current_payload = payload.clone();
        let mut step_results: Vec<StepResult> = Vec::new();
        let mut completed_order: Vec<String> = Vec::new();
        let idempotent = IdempotentCompensate::new();

        for group in &groups {
            let group_steps: Vec<&SagaStep> = group
                .iter()
                .filter_map(|id| step_map.get(id))
                .copied()
                .collect();

            let (outputs, failed) = self
                .execute_group_parallel(&group_steps, &current_payload)
                .await;

            for (step_id, result) in &outputs {
                match result {
                    Ok(output) => {
                        step_results.push(StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::ForwardDone,
                            output: Some(output.clone()),
                            error: None,
                        });
                        completed_order.push(step_id.clone());
                    }
                    Err(e) => {
                        step_results.push(StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::ForwardFailed,
                            output: None,
                            error: Some(e.to_string()),
                        });
                    }
                }
            }

            // 合并成功输出到 payload
            current_payload = merge_payload(&current_payload, &outputs);

            if let Some(failed_step) = failed {
                self.execute_compensate(
                    &step_map,
                    &completed_order,
                    &mut step_results,
                    &idempotent,
                )
                .await;

                return Ok(SagaResult {
                    tx_id,
                    success: false,
                    steps: step_results,
                    final_payload: current_payload,
                    failed_step: Some(failed_step),
                });
            }
        }

        Ok(SagaResult {
            tx_id,
            success: true,
            steps: step_results,
            final_payload: current_payload,
            failed_step: None,
        })
    }

    /// 并行执行一组无依赖步骤
    async fn execute_group_parallel(
        &self,
        steps: &[&SagaStep],
        payload: &Value,
    ) -> (Vec<(String, Result<Value, DtxError>)>, Option<String>) {
        let timeout = self.orchestrator.timeout();
        let futures: Vec<_> = steps
            .iter()
            .map(|step| {
                let step_id = step.step_id.clone();
                let forward = step.forward.clone();
                let payload = payload.clone();
                async move {
                    let exec = forward.execute(&payload);
                    let result = match timeout {
                        Some(d) => match tokio::time::timeout(d, exec).await {
                            Ok(r) => r,
                            Err(_) => Err(DtxError::Timeout(d)),
                        },
                        None => exec.await,
                    };
                    (step_id, result)
                }
            })
            .collect();

        let results = join_all(futures).await;

        let failed = results
            .iter()
            .find(|(_, r)| r.is_err())
            .map(|(id, _)| id.clone());

        (results, failed)
    }

    /// 逆序补偿已完成的步骤（幂等 + 指数退避重试）
    async fn execute_compensate(
        &self,
        step_map: &HashMap<String, &SagaStep>,
        completed_order: &[String],
        results: &mut Vec<StepResult>,
        idempotent: &IdempotentCompensate,
    ) {
        for step_id in completed_order.iter().rev() {
            if !idempotent.mark(step_id) {
                continue;
            }

            let step = match step_map.get(step_id) {
                Some(s) => *s,
                None => continue,
            };

            let mut last_err: Option<DtxError> = None;
            for attempt in 0..self.backoff.max_retries {
                if attempt > 0 {
                    tokio::time::sleep(self.backoff.interval(attempt - 1)).await;
                }

                match step.compensate.execute(&Value::Null).await {
                    Ok(output) => {
                        results.push(StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::Compensated,
                            output: Some(output),
                            error: None,
                        });
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        warn!(
                            step_id = %step_id,
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
                    step_id: step_id.clone(),
                    status: StepStatus::CompensateFailed,
                    output: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }
}

/// 合并并行步骤输出到 payload
fn merge_payload(base: &Value, outputs: &[(String, Result<Value, DtxError>)]) -> Value {
    let mut merged = if base.is_null() {
        json!({})
    } else {
        base.clone()
    };
    if let Some(obj) = merged.as_object_mut() {
        for (step_id, result) in outputs {
            if let Ok(output) = result {
                obj.insert(step_id.clone(), output.clone());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saga::{FnAction, SagaAction, SagaStep};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    #[test]
    fn test_dependency_graph_no_deps() {
        let graph = DependencyGraph::new();
        let steps = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let groups = graph.topological_groups(&steps).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn test_dependency_graph_chain() {
        let mut graph = DependencyGraph::new();
        graph.add_dependency("b", "a");
        graph.add_dependency("c", "b");
        let steps = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let groups = graph.topological_groups(&steps).unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], vec!["a".to_string()]);
        assert_eq!(groups[1], vec!["b".to_string()]);
        assert_eq!(groups[2], vec!["c".to_string()]);
    }

    #[test]
    fn test_dependency_graph_parallel_groups() {
        let mut graph = DependencyGraph::new();
        // c depends on a and b; d depends on a and b
        graph.add_dependency("c", "a");
        graph.add_dependency("c", "b");
        graph.add_dependency("d", "a");
        graph.add_dependency("d", "b");
        let steps = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let groups = graph.topological_groups(&steps).unwrap();
        assert_eq!(groups.len(), 2);
        // Group 1: a, b (no deps)
        assert_eq!(groups[0].len(), 2);
        // Group 2: c, d (depend on a, b)
        assert_eq!(groups[1].len(), 2);
    }

    #[test]
    fn test_dependency_graph_cycle_detected() {
        let mut graph = DependencyGraph::new();
        graph.add_dependency("a", "b");
        graph.add_dependency("b", "a");
        let steps = vec!["a".to_string(), "b".to_string()];
        let result = graph.topological_groups(&steps);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn test_backoff_interval_exponential() {
        let backoff = BackoffRetry::new(3, Duration::from_millis(100), 2.0);
        assert_eq!(backoff.interval(0), Duration::from_millis(100));
        assert_eq!(backoff.interval(1), Duration::from_millis(200));
        assert_eq!(backoff.interval(2), Duration::from_millis(400));
    }

    #[test]
    fn test_backoff_default() {
        let backoff = BackoffRetry::default();
        assert_eq!(backoff.max_retries, 3);
        assert_eq!(backoff.initial_interval, Duration::from_millis(50));
        assert_eq!(backoff.multiplier, 2.0);
    }

    #[test]
    fn test_idempotent_compensate() {
        let idem = IdempotentCompensate::new();
        assert!(idem.mark("step-1"));
        assert!(!idem.mark("step-1"));
        assert!(idem.is_compensated("step-1"));
        assert!(!idem.is_compensated("step-2"));
        assert!(idem.mark("step-2"));
        assert_eq!(idem.count(), 2);
    }

    #[tokio::test]
    async fn test_parallel_saga_all_success() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let f2 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(f1.clone(), false),
                make_action(c1.clone(), false),
            ),
            SagaStep::new(
                "b",
                make_action(f2.clone(), false),
                make_action(c2.clone(), false),
            ),
        ];

        let saga = ParallelSaga::new(SagaOrchestrator::new());
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(result.success);
        assert_eq!(result.failed_step, None);
        assert_eq!(f1.load(Ordering::SeqCst), 1);
        assert_eq!(f2.load(Ordering::SeqCst), 1);
        assert_eq!(c1.load(Ordering::SeqCst), 0);
        assert_eq!(c2.load(Ordering::SeqCst), 0);
        assert_eq!(result.steps.len(), 2);
    }

    #[tokio::test]
    async fn test_parallel_saga_with_dependency() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let f2 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(f1.clone(), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            SagaStep::new(
                "b",
                make_action(f2.clone(), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let mut graph = DependencyGraph::new();
        graph.add_dependency("b", "a");

        let saga = ParallelSaga::new(SagaOrchestrator::new()).with_dependency_graph(graph);
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(result.success);
        assert_eq!(f1.load(Ordering::SeqCst), 1);
        assert_eq!(f2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_parallel_saga_failure_triggers_compensate() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let f2 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(f1.clone(), false),
                make_action(c1.clone(), false),
            ),
            SagaStep::new(
                "b",
                make_action(f2.clone(), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let saga = ParallelSaga::new(SagaOrchestrator::new());
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_step.as_deref(), Some("b"));
        assert_eq!(f1.load(Ordering::SeqCst), 1);
        assert_eq!(f2.load(Ordering::SeqCst), 1);
        assert_eq!(c1.load(Ordering::SeqCst), 1, "a compensate should run");
    }

    #[tokio::test]
    async fn test_parallel_saga_compensate_idempotent() {
        let c1 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(c1.clone(), false),
            ),
            SagaStep::new(
                "b",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let saga = ParallelSaga::new(SagaOrchestrator::new());
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(
            c1.load(Ordering::SeqCst),
            1,
            "compensate should run exactly once"
        );

        let compensated_count = result
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Compensated)
            .count();
        assert_eq!(compensated_count, 1);
    }

    #[tokio::test]
    async fn test_parallel_saga_backoff_retry_success() {
        use std::sync::atomic::AtomicU32;

        let compensate_calls = Arc::new(AtomicU32::new(0));
        let calls_clone = compensate_calls.clone();

        let fail_then_succeed = Arc::new(FnAction::new(move |_payload: Value| {
            let c = calls_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    return Err(DtxError::generic("transient failure"));
                }
                Ok(Value::Null)
            }
        }));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                fail_then_succeed,
            ),
            SagaStep::new(
                "b",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let backoff = BackoffRetry::new(5, Duration::from_millis(1), 2.0);
        let saga = ParallelSaga::new(SagaOrchestrator::new()).with_backoff(backoff);
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(
            compensate_calls.load(Ordering::SeqCst),
            3,
            "first 2 fail, 3rd succeeds"
        );

        let compensated = result
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Compensated)
            .count();
        assert_eq!(compensated, 1);
    }

    #[tokio::test]
    async fn test_parallel_saga_compensate_fail_marked() {
        let c1 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(c1.clone(), true),
            ),
            SagaStep::new(
                "b",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let backoff = BackoffRetry::new(2, Duration::from_millis(1), 2.0);
        let saga = ParallelSaga::new(SagaOrchestrator::new()).with_backoff(backoff);
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(c1.load(Ordering::SeqCst), 2, "compensate retried 2 times");

        let failed_compensate = result
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::CompensateFailed)
            .count();
        assert_eq!(failed_compensate, 1);
    }

    #[tokio::test]
    async fn test_parallel_saga_empty_steps() {
        let saga = ParallelSaga::new(SagaOrchestrator::new());
        let result = saga.execute(&[], &Value::Null).await.unwrap();
        assert!(result.success);
        assert!(result.steps.is_empty());
    }

    #[tokio::test]
    async fn test_parallel_saga_parallel_execution_timing() {
        let slow_action = Arc::new(FnAction::new(|payload: Value| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(payload)
        }));

        let steps = vec![
            SagaStep::new(
                "a",
                slow_action.clone(),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            SagaStep::new(
                "b",
                slow_action.clone(),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            SagaStep::new(
                "c",
                slow_action.clone(),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let saga = ParallelSaga::new(SagaOrchestrator::new());
        let start = std::time::Instant::now();
        let result = saga.execute(&steps, &Value::Null).await.unwrap();
        let elapsed = start.elapsed();

        assert!(result.success);
        assert!(
            elapsed < Duration::from_millis(150),
            "parallel 3x50ms should be <150ms, got {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_parallel_saga_three_groups() {
        let f1 = Arc::new(AtomicUsize::new(0));
        let f2 = Arc::new(AtomicUsize::new(0));
        let f3 = Arc::new(AtomicUsize::new(0));

        let steps = vec![
            SagaStep::new(
                "a",
                make_action(f1.clone(), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            SagaStep::new(
                "b",
                make_action(f2.clone(), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            SagaStep::new(
                "c",
                make_action(f3.clone(), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let mut graph = DependencyGraph::new();
        graph.add_dependency("b", "a");
        graph.add_dependency("c", "b");

        let saga = ParallelSaga::new(SagaOrchestrator::new()).with_dependency_graph(graph);
        let result = saga.execute(&steps, &Value::Null).await.unwrap();

        assert!(result.success);
        assert_eq!(f1.load(Ordering::SeqCst), 1);
        assert_eq!(f2.load(Ordering::SeqCst), 1);
        assert_eq!(f3.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_merge_payload() {
        let base = json!({"existing": 1});
        let outputs = vec![
            ("step-a".to_string(), Ok(json!(42))),
            ("step-b".to_string(), Err(DtxError::generic("err"))),
        ];
        let merged = merge_payload(&base, &outputs);
        assert_eq!(merged["existing"], json!(1));
        assert_eq!(merged["step-a"], json!(42));
        assert!(merged.get("step-b").is_none());
    }
}
