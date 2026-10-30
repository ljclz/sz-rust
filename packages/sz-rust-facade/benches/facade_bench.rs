// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Facade criterion 基准测试（T013）
//!
//! 验证 Facade 静态调用与直接实例调用的性能差异 ≤ 5ns。

use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::json;

use sz_rust_cache_facade::{Cache as CacheInner, MemoryCacheDriver};
use sz_rust_facade::Cache;

fn bench_facade_cache_set(c: &mut Criterion) {
    // 初始化 Facade
    let inner = CacheInner::new();
    inner.register_default(MemoryCacheDriver::new());
    Cache::init(inner);

    // 直接实例
    let direct = CacheInner::new();
    direct.register_default(MemoryCacheDriver::new());

    let mut group = c.benchmark_group("cache_set");

    group.bench_function("facade", |b| {
        let mut i = 0u64;
        b.iter(|| {
            let key = format!("facade_key_{}", i);
            Cache::set(&key, json!("value"), None).unwrap();
            i += 1;
        });
    });

    group.bench_function("direct", |b| {
        let mut i = 0u64;
        b.iter(|| {
            let key = format!("direct_key_{}", i);
            direct.set(&key, json!("value"), None).unwrap();
            i += 1;
        });
    });

    group.finish();
}

fn bench_facade_cache_get(c: &mut Criterion) {
    let inner = CacheInner::new();
    inner.register_default(MemoryCacheDriver::new());
    inner.set("bench_get_key", json!("value"), None).unwrap();
    Cache::init(inner);

    let direct = CacheInner::new();
    direct.register_default(MemoryCacheDriver::new());
    direct.set("bench_get_key", json!("value"), None).unwrap();

    let mut group = c.benchmark_group("cache_get");

    group.bench_function("facade", |b| {
        b.iter(|| {
            let _: Option<serde_json::Value> = Cache::get("bench_get_key").unwrap();
        });
    });

    group.bench_function("direct", |b| {
        b.iter(|| {
            let _: Option<serde_json::Value> = direct.get("bench_get_key").unwrap();
        });
    });

    group.finish();
}

fn bench_response_construct(c: &mut Criterion) {
    use sz_rust_facade::Response;
    use sz_rust_http_facade::response::ApiResponse;

    let mut group = c.benchmark_group("response_success");

    group.bench_function("facade", |b| {
        b.iter(|| {
            let _ = Response::success(json!({"id": 1}), "ok");
        });
    });

    group.bench_function("direct", |b| {
        b.iter(|| {
            let _ = ApiResponse::success(json!({"id": 1}), "ok");
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_facade_cache_set,
    bench_facade_cache_get,
    bench_response_construct,
);
criterion_main!(benches);
