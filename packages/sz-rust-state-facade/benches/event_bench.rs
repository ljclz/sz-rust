// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Event 系统基准测试（T005）
//!
//! 验证同步分发 P99 ≤ 10μs / 异步调度 P99 ≤ 50μs。
//!
//! 运行：`cargo bench -p sz-rust-state-facade --bench event_bench`

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use serde_json::Value;
use sz_rust_state_facade::event::{ClosureListener, DispatchMode, EventDispatcher};

fn bench_dispatch_sync(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch_sync");

    for num_listeners in [1, 5, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_listeners),
            num_listeners,
            |b, &n| {
                b.iter(|| {
                    let dispatcher = EventDispatcher::new();
                    for _ in 0..n {
                        dispatcher.listen(
                            "BenchEvent",
                            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
                            false,
                        );
                    }
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        let result = dispatcher
                            .dispatch("BenchEvent", &Value::Null, DispatchMode::Sync)
                            .await
                            .unwrap();
                        assert!(result.is_success());
                    });
                });
            },
        );
    }

    group.finish();
}

fn bench_dispatch_async(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch_async");

    for num_listeners in [1, 5, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_listeners),
            num_listeners,
            |b, &n| {
                b.to_async(tokio::runtime::Runtime::new().unwrap())
                    .iter(|| async {
                        let dispatcher = EventDispatcher::new();
                        for _ in 0..n {
                            dispatcher.listen(
                                "BenchEvent",
                                Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
                                false,
                            );
                        }
                        let result = dispatcher
                            .dispatch("BenchEvent", &Value::Null, DispatchMode::Async)
                            .await
                            .unwrap();
                        assert!(result.is_success());
                    });
            },
        );
    }

    group.finish();
}

fn bench_trigger_sync(c: &mut Criterion) {
    let mut group = c.benchmark_group("trigger_sync");

    for num_listeners in [1, 5, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_listeners),
            num_listeners,
            |b, &n| {
                b.iter(|| {
                    let dispatcher = EventDispatcher::new();
                    for _ in 0..n {
                        dispatcher.listen(
                            "BenchEvent",
                            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
                            false,
                        );
                    }
                    let results = dispatcher
                        .trigger("BenchEvent", &Value::Null, false)
                        .unwrap();
                    assert_eq!(results.len(), n);
                });
            },
        );
    }

    group.finish();
}

fn bench_listen_with_priority(c: &mut Criterion) {
    let mut group = c.benchmark_group("listen_with_priority");

    for num_listeners in [1, 5, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_listeners),
            num_listeners,
            |b, &n| {
                b.iter(|| {
                    let dispatcher = EventDispatcher::new();
                    for i in 0..n {
                        dispatcher.listen_with_priority(
                            "BenchEvent",
                            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
                            n as i32 - i as i32,
                        );
                    }
                    let results = dispatcher
                        .trigger("BenchEvent", &Value::Null, false)
                        .unwrap();
                    assert_eq!(results.len(), n);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_dispatch_sync,
    bench_dispatch_async,
    bench_trigger_sync,
    bench_listen_with_priority,
);
criterion_main!(benches);
