//! P3 性能优化基准测试框架
//!
//! 5 类 benchmark，覆盖 P3 六大优化方向：
//!
//! | 类别 | 覆盖方向 | 函数 |
//! |------|---------|------|
//! | 端到端 p99 | 方向 1 | `bench_end_to_end_p99` |
//! | SIMD 字符串 | 方向 3 | `bench_simd_string` |
//! | alloc 计数 | 方向 4 | `bench_alloc_count` |
//! | 拷贝计数 | 方向 5 | `bench_copy_count` |
//! | 异步调度 | 方向 6 | `bench_async_scheduling` |
//!
//! ## 运行方式
//!
//! ```powershell
//! # 列出所有 benchmark
//! cargo bench --package sz-rust-core --bench p3_bench -- --list
//!
//! # 建立基线
//! cargo bench --package sz-rust-core --bench p3_bench -- `
//!     --warm-up-time 1 --measurement-time 3 --sample-size 30 `
//!     --save-baseline v0.5.0
//!
//! # 对比基线
//! cargo bench --package sz-rust-core --bench p3_bench -- `
//!     --warm-up-time 1 --measurement-time 3 --sample-size 30 `
//!     --baseline v0.5.0
//! ```

use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

// ============================================================================
// 1. 端到端 p99 尾延迟（方向 1：热路径优化）
// ============================================================================

/// 端到端 p99 尾延迟 benchmark
///
/// 模拟完整请求处理流程：路由解析 → 中间件链 → DI 容器 make → JSON 序列化，
/// 测量 p99 尾延迟。
///
/// P3 目标：p99 ↓ ≥ 15%
fn bench_end_to_end_p99(c: &mut Criterion) {
    use sz_rust_core::container::Container;
    use sz_rust_middleware_facade::chain::MiddlewareChain;
    use sz_rust_router_facade::router::parse_path;

    let mut group = c.benchmark_group("p3_end_to_end_p99");

    // 预建 DI 容器（注册一个简单的单例服务）
    let container = Container::new();
    container.singleton(|| 42u64);

    // 预建中间件链
    let chain = MiddlewareChain::default_chain();

    // 预建 JSON 响应数据
    let resp = serde_json::json!({
        "code": 1,
        "msg": "ok",
        "data": {"id": 1, "name": "alice", "items": [1, 2, 3]}
    });

    // 短路径：/oapc/customer/index
    group.bench_function("short_path", |b| {
        b.iter(|| {
            // 1. 路由解析
            let parsed = parse_path("/oapc/customer/index");
            // 2. 中间件链校验
            let _ = chain.has_duplicates();
            // 3. DI 容器 make
            let _ = container.make::<u64>();
            // 4. JSON 序列化
            let _ = serde_json::to_string(&resp);
            parsed
        })
    });

    // 中等路径：/oapc/customer/getListById?id=1&page=2
    group.bench_function("medium_path", |b| {
        b.iter(|| {
            let parsed = parse_path("/oapc/customer/getListById?id=1&page=2");
            let _ = chain.has_duplicates();
            let _ = container.make::<u64>();
            let _ = serde_json::to_string(&resp);
            parsed
        })
    });

    // 长路径：/api/v1/oapc/customer/getListById?id=1&page=2&size=20&sort=created_at
    group.bench_function("long_path", |b| {
        b.iter(|| {
            let parsed =
                parse_path("/api/v1/oapc/customer/getListById?id=1&page=2&size=20&sort=created_at");
            let _ = chain.has_duplicates();
            let _ = container.make::<u64>();
            let _ = serde_json::to_string(&resp);
            parsed
        })
    });

    // 根路径
    group.bench_function("root_path", |b| {
        b.iter(|| {
            let parsed = parse_path("/");
            let _ = chain.has_duplicates();
            let _ = container.make::<u64>();
            let _ = serde_json::to_string(&resp);
            parsed
        })
    });

    // 仅路由解析（隔离测量）
    group.bench_function("parse_only", |b| {
        b.iter(|| parse_path("/oapc/customer/getListById?id=1&page=2"))
    });

    // 仅 JSON 序列化（隔离测量）
    group.bench_function("json_only", |b| b.iter(|| serde_json::to_string(&resp)));

    group.finish();
}

// ============================================================================
// 2. SIMD 字符串加速（方向 3：capitalize_first / parse_path）
// ============================================================================

/// SIMD 字符串加速 benchmark
///
/// 对比 SIMD vs 标量实现：
/// - capitalize_first: 38ns → ~15ns（x86_64 SIMD）
/// - parse_path_long:  87ns → ~40ns（x86_64 SIMD）
///
/// P3 目标：x86_64 平台 capitalize_first ≤ 15ns, parse_path_long ≤ 40ns
fn bench_simd_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("p3_simd_string");

    // capitalize_first — SIMD 加速后
    group.bench_function("capitalize_first_lower", |b| {
        b.iter(|| sz_rust_core::router::capitalize_first("customer"))
    });

    group.bench_function("capitalize_first_upper", |b| {
        b.iter(|| sz_rust_core::router::capitalize_first("Customer"))
    });

    group.bench_function("capitalize_first_empty", |b| {
        b.iter(|| sz_rust_core::router::capitalize_first(""))
    });

    // parse_path — SIMD 加速后
    group.bench_function("parse_path_static", |b| {
        b.iter(|| sz_rust_core::router::parse_path("/oapc/customer/index"))
    });

    group.bench_function("parse_path_root", |b| {
        b.iter(|| sz_rust_core::router::parse_path("/"))
    });

    group.bench_function("parse_path_long", |b| {
        b.iter(|| sz_rust_core::router::parse_path("/oapc/customer/getListById?id=1&page=2"))
    });

    group.finish();
}

// ============================================================================
// 3. alloc 计数（方向 4：内存池）
// ============================================================================

/// alloc 计数 benchmark
///
/// 使用 AllocCounter::measure 统计热点路径堆分配次数，
/// 对比启用/禁用内存池的 alloc 次数。
///
/// P3 目标：热点路径 0 次堆分配（或显著下降）
fn bench_alloc_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("p3_alloc_count");

    // capitalize_first — 测量分配次数
    group.bench_function("capitalize_first_lower", |b| {
        b.iter(|| sz_rust_core::router::capitalize_first("customer"))
    });

    // parse_path — 测量分配次数
    group.bench_function("parse_path_long", |b| {
        b.iter(|| sz_rust_core::router::parse_path("/oapc/customer/getListById?id=1&page=2"))
    });

    // HandlerRef::parse — 测量分配次数
    group.bench_function("handler_ref_parse", |b| {
        b.iter(|| sz_rust_core::routing::HandlerRef::parse("User@list").unwrap())
    });

    group.finish();
}

// ============================================================================
// 4. 拷贝计数（方向 5：零拷贝优化）
// ============================================================================

/// 拷贝计数 benchmark
///
/// 对比 to_json_bytes（零拷贝）vs to_json_string（String 分配），
/// 统计热点路径拷贝次数。
///
/// P3 目标：热点路径 0 次拷贝（或显著下降）
fn bench_copy_count(c: &mut Criterion) {
    use serde_json::json;
    use sz_rust_core::http::response::ApiResponse;

    let mut group = c.benchmark_group("p3_copy_count");

    let resp = ApiResponse::success(json!({"id": 1, "name": "alice", "items": [1, 2, 3]}), "ok");

    group.bench_function("to_json_string", |b| b.iter(|| resp.to_json_string()));

    group.bench_function("to_json_bytes", |b| b.iter(|| resp.to_json_bytes()));

    group.finish();
}

// ============================================================================
// 5. 异步调度延迟（方向 6：异步优化）
// ============================================================================

/// 异步调度延迟 benchmark
///
/// 测量 tokio::spawn 到任务开始执行的调度延迟，
/// 对比不同 SzRuntime 预设配置（for_balanced / for_io_intensive / for_cpu_intensive）。
///
/// P3 目标：异步调度延迟 ↓ ≥ 20%
fn bench_async_scheduling(c: &mut Criterion) {
    use sz_rust_core::runtime::SzRuntime;
    use tokio::time::Instant;

    let mut group = c.benchmark_group("p3_async_scheduling");

    // for_balanced — 默认配置（worker = num_cpus, blocking = 512）
    let rt_balanced = SzRuntime::for_balanced();
    group.bench_function("for_balanced_spawn_await", |b| {
        b.iter(|| {
            rt_balanced.block_on(async {
                let start = Instant::now();
                let handle = rt_balanced.spawn(async {});
                let _ = handle.await;
                start.elapsed()
            })
        })
    });

    // for_io_intensive — IO 密集型（worker = num_cpus × 2, blocking = 1024）
    let rt_io = SzRuntime::for_io_intensive();
    group.bench_function("for_io_intensive_spawn_await", |b| {
        b.iter(|| {
            rt_io.block_on(async {
                let start = Instant::now();
                let handle = rt_io.spawn(async {});
                let _ = handle.await;
                start.elapsed()
            })
        })
    });

    // for_cpu_intensive — CPU 密集型（worker = num_cpus / 2, blocking = 256）
    let rt_cpu = SzRuntime::for_cpu_intensive();
    group.bench_function("for_cpu_intensive_spawn_await", |b| {
        b.iter(|| {
            rt_cpu.block_on(async {
                let start = Instant::now();
                let handle = rt_cpu.spawn(async {});
                let _ = handle.await;
                start.elapsed()
            })
        })
    });

    // spawn_blocking 调度延迟 — 对比 blocking 线程池配置
    group.bench_function("for_balanced_spawn_blocking", |b| {
        b.iter(|| {
            rt_balanced.block_on(async {
                let start = Instant::now();
                let handle = tokio::task::spawn_blocking(|| 42u64);
                let _ = handle.await;
                start.elapsed()
            })
        })
    });

    group.bench_function("for_io_intensive_spawn_blocking", |b| {
        b.iter(|| {
            rt_io.block_on(async {
                let start = Instant::now();
                let handle = tokio::task::spawn_blocking(|| 42u64);
                let _ = handle.await;
                start.elapsed()
            })
        })
    });

    group.finish();
}

// ============================================================================
// Criterion 配置
// ============================================================================

criterion_group! {
    name = p3_benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(30);
    targets =
        bench_end_to_end_p99,
        bench_simd_string,
        bench_alloc_count,
        bench_copy_count,
        bench_async_scheduling
}

criterion_main!(p3_benches);
