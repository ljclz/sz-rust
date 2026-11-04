// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P4-3 MultiDimRouter 基准性能测试

use criterion::{criterion_group, criterion_main, Criterion};
use std::collections::HashMap;
use sz_rust_ai_facade::llm::multi_dim_router::{
    FailoverChain, LoadBalanceStrategy, ModelMetrics, MultiDimRouter, MultiDimWeights,
};

fn make_metrics(n: usize) -> HashMap<String, ModelMetrics> {
    let mut metrics = HashMap::new();
    for i in 0..n {
        metrics.insert(
            format!("model-{i}"),
            ModelMetrics {
                task_match: 0.5 + (i as f64 * 0.01),
                norm_load: 0.3,
                norm_latency: 0.2 + (i as f64 * 0.01),
                norm_cost: 0.1 + (i as f64 * 0.005),
            },
        );
    }
    metrics
}

fn bench_multi_dim_router(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("multi_dim_router");

    for n in [4, 8, 16, 32] {
        group.bench_function(format!("route_{n}_models"), |b| {
            b.to_async(&rt).iter(|| async {
                let models: Vec<String> = (0..n).map(|i| format!("model-{i}")).collect();
                let weights = MultiDimWeights {
                    task: 0.3,
                    load: 0.2,
                    latency: 0.3,
                    cost: 0.2,
                };
                let failover = FailoverChain::new(models.clone()).unwrap();
                let router = MultiDimRouter::new(weights, failover)
                    .unwrap()
                    .with_load_balance(LoadBalanceStrategy::RoundRobin);
                let metrics = make_metrics(n);
                let _ = router.route(&metrics).await;
            })
        });

        group.bench_function(format!("route_with_failover_{n}_models"), |b| {
            b.to_async(&rt).iter(|| async {
                let models: Vec<String> = (0..n).map(|i| format!("model-{i}")).collect();
                let weights = MultiDimWeights {
                    task: 0.25,
                    load: 0.25,
                    latency: 0.25,
                    cost: 0.25,
                };
                let failover = FailoverChain::new(models.clone()).unwrap();
                let router = MultiDimRouter::new(weights, failover)
                    .unwrap()
                    .with_load_balance(LoadBalanceStrategy::LeastConnections);
                let metrics = make_metrics(n);
                let _ = router.route_with_failover(&metrics, &[]).await;
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_multi_dim_router);
criterion_main!(benches);
