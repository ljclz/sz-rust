//! SZ-Rust 核心性能基准测试
//!
//! 使用 criterion 0.5 进行性能测量。
//!
//! ## 运行方式
//!
//! ```powershell
//! # 建立基线
//! cargo bench --package sz-rust-core --bench core_bench -- `
//!     --warm-up-time 1 --measurement-time 3 --sample-size 30 `
//!     --save-baseline v0.1.0
//!
//! # 对比基线
//! cargo bench --package sz-rust-core --bench core_bench -- `
//!     --warm-up-time 1 --measurement-time 3 --sample-size 30 `
//!     --baseline v0.1.0
//! ```

use criterion::{criterion_group, criterion_main, Criterion};
use std::borrow::Cow;
use std::time::Duration;
use sz_rust_core::container::Container;
use sz_rust_core::middleware::chain::MiddlewareChain;
use sz_rust_core::middleware::order::MiddlewareKind;
use sz_rust_core::orm::repository::{
    EntityAttributes, InMemoryRepository, Repository, WhereCondition, WhereOp,
};
use sz_rust_core::orm::{Cache as InnerCache, MemoryCache, Value};
use sz_rust_core::router::{capitalize_first, parse_path};
use sz_rust_core::routing::{HandlerRef, HttpMethod, RouteConfig, RouteRule};
use sz_rust_middleware_facade::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use sz_rust_middleware_facade::rate_limit::{SlidingWindow, TokenBucket};
use sz_rust_orm_facade::RateLimiter;

#[derive(Clone, Debug, PartialEq, Default)]
struct BenchEntity {
    id: i64,
    name: String,
    score: i64,
    active: bool,
}

impl EntityAttributes for BenchEntity {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "name" => Some(Value::String(self.name.clone())),
            "score" => Some(Value::I64(self.score)),
            "active" => Some(Value::Bool(self.active)),
            _ => None,
        }
    }
}

// ============================================================================
// 基准测试组 1：route_matching — 路由匹配
// ============================================================================

fn bench_parse_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("route_matching");

    group.bench_function("parse_path_static", |b| {
        b.iter(|| parse_path("/oapc/customer/index"))
    });

    group.bench_function("parse_path_root", |b| b.iter(|| parse_path("/")));

    group.bench_function("parse_path_long", |b| {
        b.iter(|| parse_path("/oapc/customer/getListById?id=1&page=2"))
    });

    group.finish();
}

fn bench_handler_ref_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("handler_ref_parse");

    group.bench_function("parse_simple", |b| {
        b.iter(|| HandlerRef::parse("User@list").unwrap())
    });

    group.bench_function("parse_with_slash", |b| {
        b.iter(|| HandlerRef::parse("User/list").unwrap())
    });

    group.finish();
}

// ============================================================================
// 基准测试组 2：route_config — 路由配置加载
// ============================================================================

fn bench_route_config(c: &mut Criterion) {
    let mut group = c.benchmark_group("route_config");

    let yaml_small = r#"
routes:
  - method: GET
    path: /users
    handler: User@list
  - method: POST
    path: /users
    handler: User@create
"#;

    let yaml_medium = r#"
routes:
  - method: GET
    path: /users
    handler: User@list
  - method: POST
    path: /users
    handler: User@create
  - method: GET
    path: /users/{id}
    handler: User@show
  - method: PUT
    path: /users/{id}
    handler: User@update
  - method: DELETE
    path: /users/{id}
    handler: User@destroy
groups:
  - prefix: /api/v1
    middleware: [auth, log]
    routes:
      - method: GET
        path: /items
        handler: Item@list
      - method: POST
        path: /items
        handler: Item@create
"#;

    group.bench_function("load_yaml_small", |b| {
        b.iter(|| {
            let config = sz_rust_core::routing::load_routes_from_yaml_str(yaml_small).unwrap();
            config.flatten()
        })
    });

    group.bench_function("load_yaml_medium", |b| {
        b.iter(|| {
            let config = sz_rust_core::routing::load_routes_from_yaml_str(yaml_medium).unwrap();
            config.flatten()
        })
    });

    group.bench_function("find_conflicts", |b| {
        b.iter(|| {
            let mut config = RouteConfig::new();
            for i in 0..50 {
                config.add_route(RouteRule::new(
                    HttpMethod::GET,
                    format!("/path/{i}"),
                    format!("Ctrl@action{i}"),
                ));
            }
            config.find_conflicts()
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 3：json_serialization — JSON 序列化
// ============================================================================

fn bench_json_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_serialization");

    let small: serde_json::Value = serde_json::json!({
        "code": 1,
        "msg": "success",
        "data": { "id": 1, "name": "test" }
    });

    let medium: serde_json::Value = serde_json::json!({
        "code": 1,
        "msg": "success",
        "data": {
            "list": (0..50).map(|i| {
                serde_json::json!({
                    "id": i,
                    "name": format!("item_{i}"),
                    "price": i * 100,
                    "stock": i * 10,
                    "description": format!("Description for item {i}")
                })
            }).collect::<Vec<_>>(),
            "total": 50,
            "page": 1,
            "page_size": 50
        }
    });

    group.bench_function("serialize_small", |b| {
        b.iter(|| serde_json::to_string(&small).unwrap())
    });

    group.bench_function("serialize_medium", |b| {
        b.iter(|| serde_json::to_string(&medium).unwrap())
    });

    group.bench_function("deserialize_small", |b| {
        let json_str = serde_json::to_string(&small).unwrap();
        b.iter(|| {
            let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        })
    });

    group.bench_function("deserialize_medium", |b| {
        let json_str = serde_json::to_string(&medium).unwrap();
        b.iter(|| {
            let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 3b：capitalize_first — 首字母大写零分配路径（v0.3.3 新增）
// ============================================================================

fn bench_capitalize_first(c: &mut Criterion) {
    let mut group = c.benchmark_group("capitalize_first");

    group.bench_function("capitalize_first_already_upper", |b| {
        b.iter(|| {
            let _ = criterion::black_box(capitalize_first(criterion::black_box("Customer")));
        })
    });

    group.bench_function("capitalize_first_needs_upper", |b| {
        b.iter(|| {
            let _ = criterion::black_box(capitalize_first(criterion::black_box("customer")));
        })
    });

    group.bench_function("capitalize_first_needs_upper_24_bytes", |b| {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaa";
        b.iter(|| {
            let _ = criterion::black_box(capitalize_first(criterion::black_box(input)));
        })
    });

    group.bench_function("capitalize_first_needs_upper_25_bytes", |b| {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaaa";
        b.iter(|| {
            let _ = criterion::black_box(capitalize_first(criterion::black_box(input)));
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 3c：json_dto — DTO zero-copy 反序列化（v0.3.3 新增）
// ============================================================================

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct MediumResponse<'a> {
    code: i64,
    #[serde(borrow)]
    msg: Cow<'a, str>,
    #[serde(borrow)]
    data: MediumData<'a>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct MediumData<'a> {
    #[serde(borrow)]
    list: Vec<MediumItem<'a>>,
    total: i64,
    page: i64,
    page_size: i64,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct MediumItem<'a> {
    id: i64,
    #[serde(borrow)]
    name: Cow<'a, str>,
    price: i64,
    stock: i64,
    #[serde(borrow)]
    description: Cow<'a, str>,
}

fn bench_json_dto(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_dto");

    let medium: serde_json::Value = serde_json::json!({
        "code": 1,
        "msg": "success",
        "data": {
            "list": (0..50).map(|i| {
                serde_json::json!({
                    "id": i,
                    "name": format!("item_{i}"),
                    "price": i * 100,
                    "stock": i * 10,
                    "description": format!("Description for item {i}")
                })
            }).collect::<Vec<_>>(),
            "total": 50,
            "page": 1,
            "page_size": 50
        }
    });
    let json_str = serde_json::to_string(&medium).unwrap();

    group.bench_function("deserialize_medium_dto", |b| {
        b.iter(|| {
            let _: MediumResponse = serde_json::from_str(criterion::black_box(&json_str)).unwrap();
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 4：middleware_chain — 中间件链构建与操作
// ============================================================================

fn bench_middleware_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("middleware_chain");

    group.bench_function("default_chain", |b| b.iter(MiddlewareChain::default_chain));

    group.bench_function("push_5", |b| {
        b.iter(|| {
            MiddlewareChain::new()
                .push(MiddlewareKind::Trace)
                .push(MiddlewareKind::Cors)
                .push(MiddlewareKind::Log)
                .push(MiddlewareKind::RateLimit)
                .push(MiddlewareKind::Auth)
        })
    });

    let chain = MiddlewareChain::default_chain();
    group.bench_function("service_builder_order", |b| {
        b.iter(|| chain.service_builder_order())
    });

    group.bench_function("remove_from_auth", |b| {
        b.iter(|| chain.clone().remove_from(MiddlewareKind::Auth))
    });

    group.bench_function("has_duplicates", |b| b.iter(|| chain.has_duplicates()));

    group.bench_function("contains_auth", |b| {
        b.iter(|| chain.contains(MiddlewareKind::Auth))
    });

    group.finish();
}

// ============================================================================
// 基准测试组 5：di_container — DI 容器注册与解析
// ============================================================================

fn bench_di_container(c: &mut Criterion) {
    struct TestService;
    struct DepService;

    let mut group = c.benchmark_group("di_container");

    group.bench_function("bind_and_make_transient", |b| {
        let c = Container::new();
        c.bind::<TestService, _>(|| TestService);
        b.iter(|| {
            let _ = c.make::<TestService>();
        })
    });

    group.bench_function("singleton_reuse", |b| {
        let c = Container::new();
        c.singleton::<TestService, _>(|| TestService);
        b.iter(|| {
            let _ = c.make::<TestService>();
        })
    });

    group.bench_function("scoped_make", |b| {
        let c = Container::new();
        c.scoped::<TestService, _>(|| TestService);
        b.iter(|| {
            let _ = c.make_with_scope::<TestService>(1);
        })
    });

    group.bench_function("make_missing", |b| {
        let c = Container::new();
        b.iter(|| {
            let _ = c.make::<DepService>();
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 7：framework vs 原生（axum 风格静态路由对照，4.3 竞争力对比）
// ============================================================================

/// 原生风格：手写静态路由匹配（近似 axum matchit 的开销下限）
fn bench_vs_native(c: &mut Criterion) {
    let mut group = c.benchmark_group("framework_vs_native");

    group.bench_function("native_match_static", |b| {
        b.iter(|| {
            let uri = "/oapc/customer/index";
            let parts: Vec<&str> = uri.trim_start_matches('/').split('/').collect();
            let _ = (parts.first(), parts.get(1), parts.get(2));
        })
    });

    group.bench_function("parse_path_static_framework", |b| {
        b.iter(|| parse_path("/oapc/customer/index"))
    });

    group.bench_function("native_match_with_query", |b| {
        b.iter(|| {
            let uri = "/oapc/customer/getListById?id=1&page=2";
            let path = uri.split('?').next().unwrap_or(uri);
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let _ = (parts.first(), parts.get(1), parts.get(2));
        })
    });

    group.bench_function("parse_path_long_framework", |b| {
        b.iter(|| parse_path("/oapc/customer/getListById?id=1&page=2"))
    });

    group.finish();
}

// ============================================================================
// 基准测试组 8：rate_limiting — 限流判定（P1-5 新增）
// ============================================================================

fn bench_rate_limiting(c: &mut Criterion) {
    let mut group = c.benchmark_group("rate_limiting");

    let token_bucket = TokenBucket::new(1000, 100.0);
    group.bench_function("token_bucket_acquire", |b| {
        b.iter(|| {
            let _ = token_bucket.acquire("bench_key");
        })
    });

    let sliding_window = SlidingWindow::new(1000, Duration::from_secs(60));
    group.bench_function("sliding_window_acquire", |b| {
        b.iter(|| {
            let _ = sliding_window.acquire("bench_key");
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 9：circuit_breaker — 熔断判定（P1-5 新增）
// ============================================================================

fn bench_circuit_breaker(c: &mut Criterion) {
    let mut group = c.benchmark_group("circuit_breaker");

    let cb = CircuitBreaker::new(CircuitBreakerConfig {
        error_threshold: 0.5,
        cooldown: Duration::from_secs(30),
        probe_requests: 3,
        stat_window: Duration::from_secs(60),
    });

    group.bench_function("state_query_closed", |b| {
        b.iter(|| {
            let _ = cb.can_request();
        })
    });

    group.bench_function("record_success", |b| {
        b.iter(|| {
            cb.record_success();
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 10：orm_query_build — ORM 查询构建（W5/W6 PB-1 新增）
// ============================================================================

fn bench_orm_query_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("orm_query_build");

    group.bench_function("orm_where_condition_build_1", |b| {
        b.iter(|| {
            let _ = WhereCondition::new("id", WhereOp::Eq, Value::I64(1));
        })
    });

    group.bench_function("orm_where_condition_build_5", |b| {
        b.iter(|| {
            let conds: Vec<WhereCondition> = (1..=5)
                .map(|i| WhereCondition::new("score", WhereOp::Ge, Value::I64(i * 10)))
                .collect();
            conds
        })
    });

    group.bench_function("orm_where_condition_build_20", |b| {
        b.iter(|| {
            let conds: Vec<WhereCondition> = (1..=20)
                .map(|i| WhereCondition::new("score", WhereOp::Ge, Value::I64(i * 10)))
                .collect();
            conds
        })
    });

    let repo = InMemoryRepository::<BenchEntity>::new();
    for i in 1..=100 {
        let entity = BenchEntity {
            id: i,
            name: format!("item_{i}"),
            score: i * 10,
            active: i % 2 == 0,
        };
        let _ = repo.save(entity);
    }

    group.bench_function("orm_paginate_by_page1_size20", |b| {
        b.iter(|| repo.paginate_by(&[], 1, 20))
    });

    group.bench_function("orm_paginate_by_page10_size100", |b| {
        b.iter(|| repo.paginate_by(&[], 10, 100))
    });

    let entity = BenchEntity {
        id: 1,
        name: "test".to_string(),
        score: 100,
        active: true,
    };

    group.bench_function("orm_entity_get_attribute_all_fields", |b| {
        b.iter(|| {
            let _ = entity.get_attribute("id");
            let _ = entity.get_attribute("name");
            let _ = entity.get_attribute("score");
            let _ = entity.get_attribute("active");
            let _ = entity.get_attribute("nonexistent");
        })
    });

    group.finish();
}

// ============================================================================
// 基准测试组 11：cache_read_write — 缓存读写（W5/W6 PB-2 新增）
// ============================================================================

fn bench_cache_read_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_read_write");

    group.bench_function("cache_set_new_key", |b| {
        let cache = MemoryCache::new();
        let mut idx = 0u64;
        b.iter(|| {
            idx += 1;
            let key = format!("bench_key_{idx}");
            InnerCache::set(&cache, &key, b"value".to_vec(), None).expect("cache set");
        })
    });

    group.bench_function("cache_get_hit", |b| {
        let cache = MemoryCache::new();
        InnerCache::set(&cache, "hit_key", b"hit_value".to_vec(), None).expect("cache set");
        b.iter(|| {
            let _ = InnerCache::get(&cache, "hit_key").expect("cache get");
        })
    });

    group.bench_function("cache_get_miss", |b| {
        let cache = MemoryCache::new();
        b.iter(|| {
            let _ = InnerCache::get(&cache, "nonexistent_key").expect("cache get");
        })
    });

    group.bench_function("cache_remove_existing", |b| {
        b.iter_with_setup(
            || {
                let cache = MemoryCache::new();
                InnerCache::set(&cache, "remove_key", b"val".to_vec(), None).expect("cache set");
                cache
            },
            |cache| {
                InnerCache::delete(&cache, "remove_key").expect("cache delete");
            },
        )
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_parse_path,
    bench_handler_ref_parse,
    bench_route_config,
    bench_json_serialization,
    bench_capitalize_first,
    bench_json_dto,
    bench_middleware_chain,
    bench_di_container,
    bench_vs_native,
    bench_rate_limiting,
    bench_circuit_breaker,
    bench_orm_query_build,
    bench_cache_read_write,
);
criterion_main!(benches);
