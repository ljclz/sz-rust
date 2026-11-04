// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P4-3 ParallelSaga 基准性能测试

use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::Value;
use std::sync::Arc;
use sz_rust_distributed_tx::{
    BackoffRetry, DependencyGraph, DtxError, FnAction, ParallelSaga, SagaAction, SagaOrchestrator,
    SagaStep,
};

fn make_noop_action() -> Arc<dyn SagaAction> {
    Arc::new(FnAction::new(|payload: Value| async move { Ok(payload) }))
}

fn make_steps(n: usize) -> Vec<SagaStep> {
    (0..n)
        .map(|i| SagaStep::new(format!("step-{i}"), make_noop_action(), make_noop_action()))
        .collect()
}

fn bench_parallel_saga(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("parallel_saga");

    for n in [4, 8, 16, 32] {
        group.bench_function(format!("parallel_{n}"), |b| {
            b.to_async(&rt).iter(|| async {
                let steps = make_steps(n);
                let saga = ParallelSaga::new(SagaOrchestrator::new());
                saga.execute(&steps, &Value::Null).await.unwrap()
            })
        });

        group.bench_function(format!("sequential_{n}"), |b| {
            b.to_async(&rt).iter(|| async {
                let steps = make_steps(n);
                let mut graph = DependencyGraph::new();
                for i in 1..n {
                    graph.add_dependency(format!("step-{i}"), format!("step-{}", i - 1));
                }
                let saga = ParallelSaga::new(SagaOrchestrator::new())
                    .with_dependency_graph(graph)
                    .with_backoff(BackoffRetry::new(
                        1,
                        std::time::Duration::from_millis(1),
                        2.0,
                    ));
                saga.execute(&steps, &Value::Null).await.unwrap()
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_parallel_saga);
criterion_main!(benches);
