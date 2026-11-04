// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P4-3 多维度限流基准性能测试

use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;
use sz_rust_api_gateway::{
    MultiDimRateLimit, MultiDimRateLimitConfig, RateLimitAlgorithm, RateLimitDimensions,
    RequestContext,
};

fn bench_multi_dim_rate_limit(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_dim_rate_limit");

    let config = MultiDimRateLimitConfig {
        dimensions: RateLimitDimensions::USER
            .union(RateLimitDimensions::IP)
            .union(RateLimitDimensions::API),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        quota: 1000,
        window: Duration::from_secs(1),
    };

    group.bench_function("sliding_window_3dim", |b| {
        let rl = MultiDimRateLimit::new(config.clone());
        let ctx = RequestContext::new()
            .with_user("user-1")
            .with_ip("10.0.0.1")
            .with_api("/api/v1/test");
        b.iter(|| {
            let _ = rl.check(&ctx);
        })
    });

    let lb_config = MultiDimRateLimitConfig {
        dimensions: RateLimitDimensions::USER
            .union(RateLimitDimensions::IP)
            .union(RateLimitDimensions::API),
        algorithm: RateLimitAlgorithm::LeakyBucket,
        quota: 1000,
        window: Duration::from_secs(1),
    };

    group.bench_function("leaky_bucket_3dim", |b| {
        let rl = MultiDimRateLimit::new(lb_config.clone());
        let ctx = RequestContext::new()
            .with_user("user-1")
            .with_ip("10.0.0.1")
            .with_api("/api/v1/test");
        b.iter(|| {
            let _ = rl.check(&ctx);
        })
    });

    let global_config = MultiDimRateLimitConfig {
        dimensions: RateLimitDimensions::GLOBAL,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        quota: 10000,
        window: Duration::from_secs(1),
    };

    group.bench_function("global_only", |b| {
        let rl = MultiDimRateLimit::new(global_config.clone());
        let ctx = RequestContext::new().with_user("user-1");
        b.iter(|| {
            let _ = rl.check(&ctx);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_multi_dim_rate_limit);
criterion_main!(benches);
