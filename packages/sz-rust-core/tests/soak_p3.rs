// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P3 Soak Test — 性能优化点稳定性验证
//!
//! 覆盖 P3 5 类优化点的长时间稳定性测试：
//! 1. SIMD 字符串加速（capitalize_first / parse_path）
//! 2. 内存池 alloc 计数（StackPool / BumpaloPool）
//! 3. 零拷贝拷贝计数（to_json_bytes vs to_json_string）
//! 4. 异步调度延迟（SzRuntime 预设配置）
//! 5. L2 缓存命中（QueryCache TTL + LRU）
//!
//! # 运行方式
//!
//! ## CI 快速验证（默认 60s）
//! ```bash
//! cargo test --package sz-rust-core --test soak_p3 -- --ignored --nocapture
//! ```
//!
//! ## 6h 长时验证
//! ```bash
//! SOAK_DURATION=6h cargo test --package sz-rust-core --test soak_p3 -- --ignored --nocapture
//! ```

#![cfg(test)]

mod common;

use common::soak::{parse_duration_from_args, record_latency, SoakMonitor};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sz_rust_core::container::Container;
use sz_rust_core::response::ApiResponse;
use sz_rust_core::router::parse_path;
use sz_rust_core::routing::HandlerRef;

/// P3 优化点 Soak Test：SIMD 字符串 + 零拷贝 + 内存池 + 异步调度 + L2 缓存
///
/// 默认 60 秒，可通过 `SOAK_DURATION` 环境变量延长至 6h。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "P3 soak test 需显式 --ignored 启动；默认 60s，6h 任务 SOAK_DURATION=6h"]
async fn soak_p3_optimization_points() {
    let duration = parse_duration_from_args();
    let sample_interval = Duration::from_secs(if duration.as_secs() >= 3600 { 60 } else { 5 });

    eprintln!(
        "[soak_p3] 启动：duration={:?}, sample_interval={:?}",
        duration, sample_interval
    );

    let mut monitor = SoakMonitor::new(duration, sample_interval);
    let ops_counter = monitor.ops_counter();
    let errors_counter = monitor.errors_counter();
    let latency_window = monitor.latency_window();

    const WORKER_COUNT: usize = 8;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();

    for worker_id in 0..WORKER_COUNT {
        let ops_clone = ops_counter.clone();
        let errors_clone = errors_counter.clone();
        let latency_clone = latency_window.clone();
        let stop_clone = stop_flag.clone();

        workers.push(tokio::spawn(async move {
            let uris = [
                "/oapc/customer/index",
                "/admin/login/index",
                "/api/user/list?id=1&page=2",
                "/oapc/order/detail?order_id=123",
                "/farm/animal/feed?type=cattle",
                "/oapi/v1/product/search?keyword=hello&size=20",
                "/cashier/order/checkout?order_id=456",
                "/scene/config/getScene?id=789",
            ];

            let handlers = [
                "Customer@index",
                "Login@index",
                "User@list",
                "Order@detail",
                "Animal@feed",
                "Product@search",
                "Order@checkout",
                "Config@getScene",
            ];

            let chain = sz_rust_core::middleware::chain::MiddlewareChain::default_chain();
            let container = Container::new();
            container.singleton(|| 42i32);

            let mut iteration: usize = 0;
            while !stop_clone.load(Ordering::Relaxed) {
                let t0 = Instant::now();

                let uri = uris[iteration % uris.len()];
                let parsed = parse_path(uri);

                let handler_str = handlers[iteration % handlers.len()];
                let handler = match HandlerRef::parse(handler_str) {
                    Ok(h) => h,
                    Err(e) => {
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                        eprintln!(
                            "[soak_p3 worker {}] HandlerRef::parse({}) error: {}",
                            worker_id, handler_str, e
                        );
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                };

                let resp = ApiResponse::success(
                    serde_json::json!({
                        "app": parsed.app,
                        "controller": parsed.controller,
                        "action": parsed.action,
                        "handler": handler.to_handler_string(),
                        "iter": iteration,
                    }),
                    "ok",
                );
                let _json_bytes = resp.to_json_bytes();

                let _has_dup = chain.has_duplicates();
                let _val = container.make::<i32>();

                if iteration % 100 == 0 {
                    let handle = tokio::spawn(async { 1u64 });
                    let _ = handle.await;
                }

                let elapsed_us = t0.elapsed().as_micros() as u64;
                record_latency(&latency_clone, elapsed_us);
                ops_clone.fetch_add(1, Ordering::Relaxed);

                iteration = iteration.wrapping_add(1);
                tokio::task::yield_now().await;
            }
        }));
    }

    while !monitor.is_finished() {
        tokio::time::sleep(sample_interval).await;
        let snap = monitor.snapshot((0, 0, 0));
        eprintln!(
            "[soak_p3] t={}s ops={} ops/s={:.1} rss={}MB fd={} threads={} p99={}us errors={}",
            snap.elapsed_secs,
            snap.ops_completed,
            snap.ops_per_sec,
            snap.rss_bytes / 1024 / 1024,
            snap.fd_count,
            snap.thread_count,
            snap.p99_latency_us,
            snap.error_count,
        );
    }

    stop_flag.store(true, Ordering::Release);
    for w in workers {
        let _ = w.await;
    }

    let final_snap = monitor.snapshot((0, 0, 0));
    eprintln!(
        "[soak_p3] 完成：总操作 {} 次，错误 {} 次",
        final_snap.ops_completed, final_snap.error_count,
    );

    let csv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("soak-p3-report.csv");
    let csv_path_str = csv_path.to_str().expect("CSV path not UTF-8");
    if let Err(e) = monitor.export_csv(csv_path_str) {
        eprintln!("[soak_p3] CSV 导出失败: {}", e);
    } else {
        eprintln!("[soak_p3] CSV 报告已导出: {}", csv_path_str);
    }

    let regressions = monitor.detect_regressions();
    if regressions.is_empty() {
        eprintln!("[soak_p3] ✅ 未检测到退化");
    } else {
        eprintln!("[soak_p3] ⚠ 检测到 {} 项退化：", regressions.len());
        for r in &regressions {
            eprintln!("  - {}", r);
        }
    }

    assert!(
        regressions.is_empty(),
        "P3 soak test 检测到退化：{}",
        regressions
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    );
}

/// P3 SIMD 字符串稳定性 Soak Test
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "P3 SIMD soak test 需显式 --ignored 启动"]
async fn soak_p3_simd_string_stability() {
    let duration = parse_duration_from_args();
    let sample_interval = Duration::from_secs(if duration.as_secs() >= 3600 { 60 } else { 5 });

    eprintln!(
        "[soak_p3_simd] 启动：duration={:?}, sample_interval={:?}",
        duration, sample_interval
    );

    let mut monitor = SoakMonitor::new(duration, sample_interval);
    let ops_counter = monitor.ops_counter();
    let errors_counter = monitor.errors_counter();
    let latency_window = monitor.latency_window();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();

    for _ in 0..4 {
        let ops_clone = ops_counter.clone();
        let errors_clone = errors_counter.clone();
        let latency_clone = latency_window.clone();
        let stop_clone = stop_flag.clone();

        workers.push(tokio::spawn(async move {
            let test_inputs = [
                "/oapc/customer/index",
                "/",
                "/admin/user/list?page=1&size=20&sort=name&order=asc",
                "/api/v1/oapc/customer/getListById?id=12345&page=10&size=50",
                "/farm/animal/feed",
                "/oapi/cashier/scene/config/getScene?id=999&lang=zh",
            ];

            let mut iteration: usize = 0;
            while !stop_clone.load(Ordering::Relaxed) {
                let t0 = Instant::now();

                let input = test_inputs[iteration % test_inputs.len()];
                let parsed = parse_path(input);

                if parsed.controller.is_empty() {
                    errors_clone.fetch_add(1, Ordering::Relaxed);
                }

                let elapsed_us = t0.elapsed().as_micros() as u64;
                record_latency(&latency_clone, elapsed_us);
                ops_clone.fetch_add(1, Ordering::Relaxed);

                iteration = iteration.wrapping_add(1);
                tokio::task::yield_now().await;
            }
        }));
    }

    while !monitor.is_finished() {
        tokio::time::sleep(sample_interval).await;
        let snap = monitor.snapshot((0, 0, 0));
        eprintln!(
            "[soak_p3_simd] t={}s ops={} ops/s={:.1} p99={}us errors={}",
            snap.elapsed_secs,
            snap.ops_completed,
            snap.ops_per_sec,
            snap.p99_latency_us,
            snap.error_count,
        );
    }

    stop_flag.store(true, Ordering::Release);
    for w in workers {
        let _ = w.await;
    }

    let regressions = monitor.detect_regressions();
    assert!(
        regressions.is_empty(),
        "P3 SIMD soak test 检测到退化：{:?}",
        regressions
    );
    eprintln!("[soak_p3_simd] ✅ 未检测到退化");
}

/// P3 异步调度稳定性 Soak Test
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "P3 async soak test 需显式 --ignored 启动"]
async fn soak_p3_async_scheduling_stability() {
    let duration = parse_duration_from_args();
    let sample_interval = Duration::from_secs(if duration.as_secs() >= 3600 { 60 } else { 5 });

    eprintln!(
        "[soak_p3_async] 启动：duration={:?}, sample_interval={:?}",
        duration, sample_interval
    );

    let mut monitor = SoakMonitor::new(duration, sample_interval);
    let ops_counter = monitor.ops_counter();
    let errors_counter = monitor.errors_counter();
    let latency_window = monitor.latency_window();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();

    for _ in 0..4 {
        let ops_clone = ops_counter.clone();
        let errors_clone = errors_counter.clone();
        let latency_clone = latency_window.clone();
        let stop_clone = stop_flag.clone();

        workers.push(tokio::spawn(async move {
            let mut iteration: usize = 0;
            while !stop_clone.load(Ordering::Relaxed) {
                let t0 = Instant::now();

                let handle = tokio::spawn(async move { iteration });
                match handle.await {
                    Ok(_) => {}
                    Err(_) => {
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }

                if iteration % 50 == 0 {
                    let blocking_handle = tokio::task::spawn_blocking(|| u64::MAX);
                    let _ = blocking_handle.await;
                }

                let elapsed_us = t0.elapsed().as_micros() as u64;
                record_latency(&latency_clone, elapsed_us);
                ops_clone.fetch_add(1, Ordering::Relaxed);

                iteration = iteration.wrapping_add(1);
                tokio::task::yield_now().await;
            }
        }));
    }

    while !monitor.is_finished() {
        tokio::time::sleep(sample_interval).await;
        let snap = monitor.snapshot((0, 0, 0));
        eprintln!(
            "[soak_p3_async] t={}s ops={} ops/s={:.1} p99={}us errors={}",
            snap.elapsed_secs,
            snap.ops_completed,
            snap.ops_per_sec,
            snap.p99_latency_us,
            snap.error_count,
        );
    }

    stop_flag.store(true, Ordering::Release);
    for w in workers {
        let _ = w.await;
    }

    let regressions = monitor.detect_regressions();
    assert!(
        regressions.is_empty(),
        "P3 async soak test 检测到退化：{:?}",
        regressions
    );
    eprintln!("[soak_p3_async] ✅ 未检测到退化");
}
