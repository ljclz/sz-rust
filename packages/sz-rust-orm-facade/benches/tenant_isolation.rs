// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T13.1 租户隔离性能基准 — spec 4.1.1/4.1.2/4.1.3/4.1.4
//!
//! 基准 1: 租户解析中间件开销（P99 ≤ 0.5ms，spec 4.1.1）
//! 基准 2: tenant_id 条件注入开销（P99 ≤ 0.1ms，spec 4.1.2）
//! 基准 3: 租户管理 API 响应时间（P99 ≤ 50ms，spec 4.1.3）
//! 基准 4: 租户配置读取响应时间（P99 ≤ 20ms，含缓存命中，spec 4.1.4）

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;

use sz_rust_orm_facade::data_scope::ext::DataScopeExt;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::repository::WhereCondition;
use sz_rust_orm_facade::tenant::config::{TenantConfigRegistry, TenantConfigType};
use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
use sz_rust_orm_facade::tenant::resolver::{TenantRequest, TenantResolver};
use sz_rust_orm_facade::tenant::scope_ext::TenantScopeExt;
use sz_rust_orm_facade::tenant::scoped_table::{TenantScopedTable, TenantScopedTableRegistry};

use async_trait::async_trait;

#[derive(Debug)]
struct MockQueryBuilder {
    conditions: Vec<WhereCondition>,
}

#[async_trait]
impl DataScopeExt for MockQueryBuilder {
    fn with_data_scope_conditions(mut self, conditions: &[WhereCondition]) -> Self {
        self.conditions.extend(conditions.iter().cloned());
        self
    }
}

struct MockRequest {
    headers: std::collections::HashMap<String, String>,
    jwt_tenant_id: Option<i64>,
}

impl MockRequest {
    fn with_header_tenant(tenant_id: i64) -> Self {
        let mut headers = std::collections::HashMap::new();
        headers.insert("x-tenant-id".to_string(), tenant_id.to_string());
        Self {
            headers,
            jwt_tenant_id: None,
        }
    }
}

impl TenantRequest for MockRequest {
    fn get_header(&self, name: &str) -> Option<String> {
        self.headers.get(&name.to_lowercase()).cloned()
    }
    fn get_path_param(&self, _name: &str) -> Option<String> {
        None
    }
    fn get_jwt_tenant_id(&self) -> Option<i64> {
        self.jwt_tenant_id
    }
}

fn bench_tenant_resolve(c: &mut Criterion) {
    let resolver = Arc::new(TenantResolver::new(None));
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("tenant_resolve");
    group.bench_function("header_resolve", |b| {
        b.to_async(&rt).iter(|| {
            let resolver = resolver.clone();
            async move {
                let req = MockRequest::with_header_tenant(42);
                resolver.resolve(&req).await
            }
        })
    });
    group.finish();
}

fn bench_tenant_scope_injection(c: &mut Criterion) {
    let registry = Arc::new(TenantScopedTableRegistry::new());
    registry.register(TenantScopedTable::new("orders"));
    let metrics = Arc::new(DataScopeMetrics::new());

    let ctx = TenantContext::new(1, false, TenantResolveSource::Header);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("tenant_scope_injection");
    group.bench_with_input(
        BenchmarkId::new("condition_inject", "orders"),
        &ctx,
        |b, ctx| {
            b.to_async(&rt).iter(|| {
                let builder = MockQueryBuilder { conditions: vec![] };
                builder.tenant_scope_async(ctx, &registry, "orders", &metrics)
            })
        },
    );
    group.finish();
}

fn bench_tenant_config_read(c: &mut Criterion) {
    let registry = Arc::new(TenantConfigRegistry::new());
    registry
        .set_tenant_config(1, "theme", "dark", TenantConfigType::String)
        .unwrap();

    let mut group = c.benchmark_group("tenant_config_read");
    group.bench_function("cache_hit", |b| {
        b.iter(|| {
            let _ = registry.get(1, "theme");
        })
    });

    registry
        .set_global_config("locale", "en", TenantConfigType::String)
        .unwrap();
    group.bench_function("global_inheritance", |b| {
        b.iter(|| {
            let _ = registry.get(2, "locale");
        })
    });
    group.finish();
}

fn bench_tenant_create(c: &mut Criterion) {
    let mut group = c.benchmark_group("tenant_management");
    group.bench_function("create_tenant", |b| {
        b.iter_with_setup(TenantRecordRegistry::new, |registry| {
            registry.create(format!(
                "bench_{}",
                std::time::Instant::now().elapsed().as_nanos()
            ))
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_tenant_resolve,
    bench_tenant_scope_injection,
    bench_tenant_config_read,
    bench_tenant_create,
);
criterion_main!(benches);
