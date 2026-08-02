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
use sz_rust_core::container::Container;
use sz_rust_core::middleware::chain::MiddlewareChain;
use sz_rust_core::middleware::order::MiddlewareKind;
use sz_rust_core::router::parse_path;
use sz_rust_core::routing::{HandlerRef, HttpMethod, RouteConfig, RouteRule};

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
// 基准测试组 4：middleware_chain — 中间件链构建与操作
// ============================================================================

fn bench_middleware_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("middleware_chain");

    group.bench_function("default_chain", |b| {
        b.iter(|| MiddlewareChain::default_chain())
    });

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

criterion_group!(
    benches,
    bench_parse_path,
    bench_handler_ref_parse,
    bench_route_config,
    bench_json_serialization,
    bench_middleware_chain,
    bench_di_container,
);
criterion_main!(benches);
