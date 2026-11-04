// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 连接池压测基准 — 测量不同 pool_size × concurrency 组合下的获取延迟和利用率
//!
//! 运行: cargo bench --package sz-rust-core --bench pool_tuning

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

async fn bench_pool_acquire(pool_size: usize, concurrency: usize) -> (Duration, f64) {
    let semaphore = Arc::new(Semaphore::new(pool_size));
    let start = Instant::now();

    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let permit = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = permit.acquire().await.unwrap();
            tokio::task::yield_now().await;
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    let elapsed = start.elapsed();
    let utilization = (concurrency as f64 / pool_size as f64).min(1.0);
    (elapsed, utilization)
}

fn bench_pool_tuning(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool_sizes = [10usize, 20, 50, 100];
    let concurrencies = [50usize, 100, 200, 500];

    let mut group = c.benchmark_group("pool_tuning");

    for &pool_size in &pool_sizes {
        for &concurrency in &concurrencies {
            group.throughput(Throughput::Elements(concurrency as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("pool={pool_size}"), concurrency),
                &concurrency,
                |b, &_conc| {
                    b.iter(|| {
                        rt.block_on(async { bench_pool_acquire(pool_size, concurrency).await })
                    });
                },
            );
        }
    }

    group.finish();
}

fn bench_utilization_curve(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let configs: &[(usize, usize)] = &[
        (10, 50),
        (10, 100),
        (20, 100),
        (20, 200),
        (50, 200),
        (50, 500),
        (100, 500),
        (100, 100),
    ];

    let mut group = c.benchmark_group("pool_utilization_curve");

    for &(pool_size, concurrency) in configs {
        let utilization = (concurrency as f64 / pool_size as f64).min(1.0);
        group.bench_function(
            format!(
                "pool={pool_size}_conc={concurrency}_util={:.2}",
                utilization
            ),
            |b| {
                b.iter(|| rt.block_on(async { bench_pool_acquire(pool_size, concurrency).await }));
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_pool_tuning, bench_utilization_curve);
criterion_main!(benches);
