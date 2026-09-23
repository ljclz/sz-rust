// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 队列压测验证（T009）
//!
//! 验证 Redis 后端吞吐 ≥ 10,000 Job/s、投递延迟 ≤ 1ms。
//! 需要Redis 测试环境，用 `--ignored` 标记。
//!
//! 运行：`cargo test -p sz-rust-orm-facade --test queue_stress -- --ignored`

#![cfg(test)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use sz_rust_orm_facade::{QueueBackend, RedisQueueBackend};

/// 验证 Redis 后端投递吞吐 ≥ 10,000 Job/s
#[tokio::test]
#[ignore]
async fn stress_redis_push_throughput() {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let client = redis::Client::open(redis_url.as_str()).expect("failed to create Redis client");
    let conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("failed to connect Redis");

    let backend: Arc<dyn QueueBackend> = Arc::new(RedisQueueBackend::new(conn, "stress_test"));
    backend.init_schema().await.unwrap();

    let total_jobs: u32 = 10_000;
    let start = Instant::now();

    for i in 0..total_jobs {
        backend
            .push(
                "stress",
                json!({"index": i}),
                None,
                sz_rust_orm_facade::jobs::now_ms(),
            )
            .await
            .unwrap();
    }

    let elapsed = start.elapsed();
    let jobs_per_sec = total_jobs as f64 / elapsed.as_secs_f64();

    println!(
        "Pushed {} jobs in {:?} ({:.0} jobs/s)",
        total_jobs, elapsed, jobs_per_sec
    );

    assert!(
        jobs_per_sec >= 10_000.0,
        "throughput {jobs_per_sec:.0} < 10,000 jobs/s"
    );
}

/// 验证投递延迟 ≤ 1ms（单次 push 耗时）
#[tokio::test]
#[ignore]
async fn stress_redis_push_latency() {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let client = redis::Client::open(redis_url.as_str()).expect("failed to create Redis client");
    let conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("failed to connect Redis");

    let backend: Arc<dyn QueueBackend> = Arc::new(RedisQueueBackend::new(conn, "latency_test"));
    backend.init_schema().await.unwrap();

    let warmup = 100;
    for i in 0..warmup {
        backend
            .push(
                "warmup",
                json!({"i": i}),
                None,
                sz_rust_orm_facade::jobs::now_ms(),
            )
            .await
            .unwrap();
    }

    let samples = 1000;
    let mut max_latency = Duration::ZERO;
    for i in 0..samples {
        let start = Instant::now();
        backend
            .push(
                "latency",
                json!({"i": i}),
                None,
                sz_rust_orm_facade::jobs::now_ms(),
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();
        if elapsed > max_latency {
            max_latency = elapsed;
        }
    }

    println!(
        "Max push latency over {} samples: {:?} ({:.3}ms)",
        samples,
        max_latency,
        max_latency.as_secs_f64() * 1000.0
    );

    assert!(
        max_latency <= Duration::from_millis(1),
        "max latency {:?} > 1ms",
        max_latency
    );
}
