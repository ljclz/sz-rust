// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P3-1 ParallelSaga 集成测试

#![cfg(feature = "dtx-parallel")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use sz_rust_distributed_tx::{
    BackoffRetry, DependencyGraph, DtxError, FnAction, IdempotentCompensate, ParallelSaga,
    SagaAction, SagaOrchestrator, SagaStep, StepStatus,
};

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
async fn test_parallel_no_dependency_runs_concurrently() {
    let slow_action = Arc::new(FnAction::new(|payload: Value| async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        Ok(payload)
    }));
    let noop = make_action(Arc::new(AtomicUsize::new(0)), false);

    let steps = vec![
        SagaStep::new("pa", slow_action.clone(), noop.clone()),
        SagaStep::new("pb", slow_action.clone(), noop.clone()),
        SagaStep::new("pc", slow_action.clone(), noop.clone()),
        SagaStep::new("pd", slow_action.clone(), noop.clone()),
    ];

    let saga = ParallelSaga::new(SagaOrchestrator::new());
    let start = Instant::now();
    let result = saga.execute(&steps, &Value::Null).await.unwrap();
    let elapsed = start.elapsed();

    assert!(result.success);
    assert!(
        elapsed < Duration::from_millis(200),
        "4 parallel 80ms should be <200ms, got {:?}",
        elapsed
    );
    assert_eq!(result.steps.len(), 4);
    for step in &result.steps {
        assert_eq!(step.status, StepStatus::ForwardDone);
    }
}

#[tokio::test]
async fn test_parallel_failure_compensates_all_completed() {
    let f1 = Arc::new(AtomicUsize::new(0));
    let f2 = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "pa",
            make_action(f1.clone(), false),
            make_action(c1.clone(), false),
        ),
        SagaStep::new(
            "pb",
            make_action(f2.clone(), false),
            make_action(c2.clone(), false),
        ),
        SagaStep::new(
            "pc",
            make_action(Arc::new(AtomicUsize::new(0)), true),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
    ];

    let saga = ParallelSaga::new(SagaOrchestrator::new());
    let result = saga.execute(&steps, &Value::Null).await.unwrap();

    assert!(!result.success);
    assert_eq!(result.failed_step.as_deref(), Some("pc"));
    assert_eq!(f1.load(Ordering::SeqCst), 1, "pa forward ran");
    assert_eq!(f2.load(Ordering::SeqCst), 1, "pb forward ran");
    assert_eq!(c1.load(Ordering::SeqCst), 1, "pa compensate ran");
    assert_eq!(c2.load(Ordering::SeqCst), 1, "pb compensate ran");
}

#[tokio::test]
async fn test_parallel_compensate_idempotent_no_side_effect() {
    let compensate_count = Arc::new(AtomicUsize::new(0));
    let cc = compensate_count.clone();

    let idempotent_compensate = Arc::new(FnAction::new(move |payload: Value| {
        let c = cc.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(payload)
        }
    }));

    let steps = vec![
        SagaStep::new(
            "pa",
            make_action(Arc::new(AtomicUsize::new(0)), false),
            idempotent_compensate.clone(),
        ),
        SagaStep::new(
            "pb",
            make_action(Arc::new(AtomicUsize::new(0)), true),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
    ];

    let saga = ParallelSaga::new(SagaOrchestrator::new());
    let result = saga.execute(&steps, &Value::Null).await.unwrap();

    assert!(!result.success);
    assert_eq!(
        compensate_count.load(Ordering::SeqCst),
        1,
        "compensate should run exactly once (idempotent)"
    );

    let compensated = result
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Compensated)
        .count();
    assert_eq!(compensated, 1);
}

#[tokio::test]
async fn test_parallel_transient_failure_retry_succeeds() {
    use std::sync::atomic::AtomicU32;

    let compensate_calls = Arc::new(AtomicU32::new(0));
    let cc = compensate_calls.clone();

    let fail_twice_then_succeed = Arc::new(FnAction::new(move |_payload: Value| {
        let c = cc.clone();
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
            "pa",
            make_action(Arc::new(AtomicUsize::new(0)), false),
            fail_twice_then_succeed,
        ),
        SagaStep::new(
            "pb",
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
async fn test_parallel_with_dependency_chain() {
    let f1 = Arc::new(AtomicUsize::new(0));
    let f2 = Arc::new(AtomicUsize::new(0));
    let f3 = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "pa",
            make_action(f1.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
        SagaStep::new(
            "pb",
            make_action(f2.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
        SagaStep::new(
            "pc",
            make_action(f3.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
    ];

    let mut graph = DependencyGraph::new();
    graph.add_dependency("pb", "pa");
    graph.add_dependency("pc", "pb");

    let saga = ParallelSaga::new(SagaOrchestrator::new()).with_dependency_graph(graph);
    let result = saga.execute(&steps, &Value::Null).await.unwrap();

    assert!(result.success);
    assert_eq!(f1.load(Ordering::SeqCst), 1);
    assert_eq!(f2.load(Ordering::SeqCst), 1);
    assert_eq!(f3.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_parallel_mixed_parallel_and_sequential() {
    let f_a = Arc::new(AtomicUsize::new(0));
    let f_b = Arc::new(AtomicUsize::new(0));
    let f_c = Arc::new(AtomicUsize::new(0));
    let f_d = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "pa",
            make_action(f_a.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
        SagaStep::new(
            "pb",
            make_action(f_b.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
        SagaStep::new(
            "pc",
            make_action(f_c.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
        SagaStep::new(
            "pd",
            make_action(f_d.clone(), false),
            make_action(Arc::new(AtomicUsize::new(0)), false),
        ),
    ];

    // Group1: pa, pb (parallel); Group2: pc, pd (parallel, depend on pa+pb)
    let mut graph = DependencyGraph::new();
    graph.add_dependency("pc", "pa");
    graph.add_dependency("pc", "pb");
    graph.add_dependency("pd", "pa");
    graph.add_dependency("pd", "pb");

    let saga = ParallelSaga::new(SagaOrchestrator::new()).with_dependency_graph(graph);
    let result = saga.execute(&steps, &Value::Null).await.unwrap();

    assert!(result.success);
    assert_eq!(f_a.load(Ordering::SeqCst), 1);
    assert_eq!(f_b.load(Ordering::SeqCst), 1);
    assert_eq!(f_c.load(Ordering::SeqCst), 1);
    assert_eq!(f_d.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_parallel_compensate_failure_marked_for_manual() {
    let c1 = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "pa",
            make_action(Arc::new(AtomicUsize::new(0)), false),
            make_action(c1.clone(), true),
        ),
        SagaStep::new(
            "pb",
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
    assert_eq!(failed_compensate, 1, "should mark for manual intervention");
}

#[tokio::test]
async fn test_parallel_empty_steps() {
    let saga = ParallelSaga::new(SagaOrchestrator::new());
    let result = saga.execute(&[], &Value::Null).await.unwrap();
    assert!(result.success);
    assert!(result.steps.is_empty());
}

#[tokio::test]
async fn test_parallel_payload_merged() {
    let action_a = Arc::new(FnAction::new(|_p: Value| async move {
        Ok(json!({"result_a": 100}))
    }));
    let action_b = Arc::new(FnAction::new(|_p: Value| async move {
        Ok(json!({"result_b": 200}))
    }));
    let noop = make_action(Arc::new(AtomicUsize::new(0)), false);

    let steps = vec![
        SagaStep::new("pa", action_a, noop.clone()),
        SagaStep::new("pb", action_b, noop),
    ];

    let saga = ParallelSaga::new(SagaOrchestrator::new());
    let result = saga.execute(&steps, &json!({"init": true})).await.unwrap();

    assert!(result.success);
    assert_eq!(result.final_payload["init"], json!(true));
    assert_eq!(result.final_payload["pa"]["result_a"], json!(100));
    assert_eq!(result.final_payload["pb"]["result_b"], json!(200));
}

#[test]
fn test_dependency_graph_cycle_error() {
    let mut graph = DependencyGraph::new();
    graph.add_dependency("a", "b");
    graph.add_dependency("b", "c");
    graph.add_dependency("c", "a");
    let steps = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let result = graph.topological_groups(&steps);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("cycle"));
}

#[test]
fn test_idempotent_compensate_dedup() {
    let idem = IdempotentCompensate::new();
    assert!(idem.mark("s1"), "first mark should return true");
    assert!(!idem.mark("s1"), "second mark should return false");
    assert!(idem.mark("s2"), "different step should return true");
    assert_eq!(idem.count(), 2);
}

#[test]
fn test_backoff_exponential_intervals() {
    let backoff = BackoffRetry::new(4, Duration::from_millis(100), 2.0);
    assert_eq!(backoff.interval(0), Duration::from_millis(100));
    assert_eq!(backoff.interval(1), Duration::from_millis(200));
    assert_eq!(backoff.interval(2), Duration::from_millis(400));
    assert_eq!(backoff.interval(3), Duration::from_millis(800));
}
