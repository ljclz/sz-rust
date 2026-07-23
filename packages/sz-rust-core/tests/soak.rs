//! Soak Test 测试入口（Web 框架场景）
//!
//! 长时间中等压力持续运行路由解析 + JSON 序列化，检测内存泄漏、
//! 句柄泄漏、慢退化等问题。不使用数据库，专注于 Web 框架核心路径。
//!
//! # 运行方式
//!
//! ## CI 快速验证（默认 60s）
//! ```bash
//! cargo test --package sz-rust-core --test soak soak_web_framework_steady_state -- --ignored --nocapture
//! ```
//!
//! ## 周末长时验证（24h）
//! ```bash
//! SOAK_DURATION=24h cargo test --package sz-rust-core --test soak soak_web_framework_steady_state -- --ignored --nocapture
//! ```
//!
//! ## 自定义时长
//! ```bash
//! SOAK_DURATION=5m cargo test --package sz-rust-core --test soak soak_web_framework_steady_state -- --ignored --nocapture
//! ```
//!
//! ## 冒烟测试（10s，每次 commit 运行）
//! ```bash
//! cargo test --package sz-rust-core --test soak soak_smoke_10s -- --nocapture --test-threads=1
//! ```
//!
//! # 监控指标
//!
//! - RSS / fd_count / thread_count
//! - ops_per_sec / p99_latency_us / error_count
//!
//! # 退化检测
//!
//! - RSS 增长 > 50MB → 内存泄漏
//! - fd_count 增长 > 10 → 句柄泄漏
//! - ops_per_sec 衰减 > 10% → 性能退化
//! - p99_latency 增长 > 2x → 慢退化
//! - error_count > 0 → 偶发错误

#![cfg(test)]

mod common;

use common::soak::{parse_duration_from_args, record_latency, SoakMonitor, SoakRegression};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sz_rust_core::response::ApiResponse;
use sz_rust_core::router::parse_path;
use sz_rust_core::routing::HandlerRef;

/// 完整 Soak Test：长时间运行路由解析 + JSON 序列化
///
/// 默认 60 秒（CI 验证），可通过 `SOAK_DURATION` 环境变量延长至 24h。
///
/// # Worker 操作
///
/// 每个 worker 线程循环执行：
/// 1. `parse_path` 解析 URI 路径为 (app, controller, action) 三元组
/// 2. `HandlerRef::parse` 解析 "Controller@action" 字符串
/// 3. `ApiResponse::success` + `to_json_string` 序列化 JSON 响应
/// 4. 记录操作延迟和计数
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "soak test 需显式 --ignored 启动；默认 60s，24h 任务 SOAK_DURATION=24h"]
async fn soak_web_framework_steady_state() {
    let duration = parse_duration_from_args();
    let sample_interval = Duration::from_secs(if duration.as_secs() >= 3600 {
        60 // ≥1h 时每分钟采样
    } else {
        5 // 短时测试每 5s 采样
    });

    eprintln!(
        "[soak] 启动：duration={:?}, sample_interval={:?}",
        duration, sample_interval
    );

    // 创建 SoakMonitor
    let mut monitor = SoakMonitor::new(duration, sample_interval);
    let ops_counter = monitor.ops_counter();
    let errors_counter = monitor.errors_counter();
    let latency_window = monitor.latency_window();

    // 工作线程数：8 个并发 worker
    const WORKER_COUNT: usize = 8;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();

    for worker_id in 0..WORKER_COUNT {
        let ops_clone = ops_counter.clone();
        let errors_clone = errors_counter.clone();
        let latency_clone = latency_window.clone();
        let stop_clone = stop_flag.clone();

        workers.push(tokio::spawn(async move {
            // 多组测试 URI，模拟真实路由场景
            let uris = [
                "/oapc/customer/index",
                "/admin/login/index",
                "/api/user/list",
                "/oapc/order/detail",
                "/farm/animal/feed",
            ];
            let handlers = [
                "Customer@index",
                "Login@index",
                "User@list",
                "Order@detail",
                "Animal@feed",
            ];

            let mut iteration: usize = 0;
            while !stop_clone.load(Ordering::Relaxed) {
                let t0 = Instant::now();

                // 1. 路由路径解析
                let uri = uris[iteration % uris.len()];
                let parsed = parse_path(uri);

                // 2. Handler 引用解析
                let handler_str = handlers[iteration % handlers.len()];
                let handler = match HandlerRef::parse(handler_str) {
                    Ok(h) => h,
                    Err(e) => {
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                        eprintln!(
                            "[soak worker {}] HandlerRef::parse({}) error: {}",
                            worker_id, handler_str, e
                        );
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                };

                // 3. JSON 响应序列化（模拟 Controller 返回 ApiResponse）
                let resp = ApiResponse::success(
                    serde_json::json!({
                        "app": parsed.app,
                        "controller": parsed.controller,
                        "action": parsed.action,
                        "handler": handler.to_handler_string(),
                    }),
                    "ok",
                );
                let _json = resp.to_json_string();

                // 4. 记录延迟和计数
                let elapsed_us = t0.elapsed().as_micros() as u64;
                record_latency(&latency_clone, elapsed_us);
                ops_clone.fetch_add(1, Ordering::Relaxed);

                iteration = iteration.wrapping_add(1);
                // 让出调度器，避免 CPU 密集循环饿死采样任务
                tokio::task::yield_now().await;
            }
        }));
    }

    // 主线程：周期性采样
    while !monitor.is_finished() {
        tokio::time::sleep(sample_interval).await;
        // Web 框架无连接池，pool_status 始终为 (0, 0, 0)
        let snap = monitor.snapshot((0, 0, 0));
        eprintln!(
            "[soak] t={}s ops={} ops/s={:.1} rss={}MB fd={} threads={} p99={}us errors={}",
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

    // 停止工作线程
    stop_flag.store(true, Ordering::Release);
    for w in workers {
        let _ = w.await;
    }

    // 采集最终快照
    let final_snap = monitor.snapshot((0, 0, 0));
    eprintln!(
        "[soak] 完成：总操作 {} 次，错误 {} 次",
        final_snap.ops_completed, final_snap.error_count,
    );

    // 导出 CSV 报告
    // cargo test 工作目录是包目录（packages/sz-rust-core），target 在 workspace 根
    let csv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("soak-report.csv");
    let csv_path_str = csv_path.to_str().expect("CSV path not UTF-8");
    if let Err(e) = monitor.export_csv(csv_path_str) {
        eprintln!("[soak] CSV 导出失败: {}", e);
    } else {
        eprintln!("[soak] CSV 报告已导出: {}", csv_path_str);
    }

    // 退化检测
    let regressions = monitor.detect_regressions();
    if regressions.is_empty() {
        eprintln!("[soak] ✅ 未检测到退化");
    } else {
        eprintln!("[soak] ⚠ 检测到 {} 项退化：", regressions.len());
        for r in &regressions {
            eprintln!("  - {}", r);
        }
    }

    // 断言：无任何退化（CI 验证标准）
    assert!(
        regressions.is_empty(),
        "Soak test 检测到退化：{}",
        regressions
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    );
}

/// 短时 Soak 冒烟测试（10s）：验证 Soak 框架自身正确
///
/// 不需要 --ignored，每次 commit 都运行。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soak_smoke_10s() {
    let duration = Duration::from_secs(10);
    let sample_interval = Duration::from_secs(2);

    let mut monitor = SoakMonitor::new(duration, sample_interval);
    let ops = monitor.ops_counter();
    let errors = monitor.errors_counter();
    let latency = monitor.latency_window();
    let stop = Arc::new(AtomicBool::new(false));

    let mut workers = Vec::new();
    for _ in 0..4 {
        let ops_c = ops.clone();
        let err_c = errors.clone();
        let lat_c = latency.clone();
        let stop_c = stop.clone();

        workers.push(tokio::spawn(async move {
            let uris = [
                "/oapc/customer/index",
                "/admin/login/index",
                "/api/user/list",
            ];
            let handlers = ["Customer@index", "Login@index", "User@list"];

            let mut iteration: usize = 0;
            while !stop_c.load(Ordering::Relaxed) {
                let t0 = Instant::now();

                let uri = uris[iteration % uris.len()];
                let parsed = parse_path(uri);

                let handler_str = handlers[iteration % handlers.len()];
                if let Ok(handler) = HandlerRef::parse(handler_str) {
                    let resp = ApiResponse::success(
                        serde_json::json!({
                            "app": parsed.app,
                            "controller": parsed.controller,
                            "action": parsed.action,
                            "handler": handler.to_handler_string(),
                        }),
                        "ok",
                    );
                    let _json = resp.to_json_string();
                    record_latency(&lat_c, t0.elapsed().as_micros() as u64);
                    ops_c.fetch_add(1, Ordering::Relaxed);
                } else {
                    err_c.fetch_add(1, Ordering::Relaxed);
                }

                iteration = iteration.wrapping_add(1);
                // 让出调度器，避免 CPU 密集循环饿死采样任务
                tokio::task::yield_now().await;
            }
        }));
    }

    while !monitor.is_finished() {
        tokio::time::sleep(sample_interval).await;
        let snap = monitor.snapshot((0, 0, 0));
        eprintln!(
            "[soak-smoke] t={}s ops={} rss={}MB p99={}us errors={}",
            snap.elapsed_secs,
            snap.ops_completed,
            snap.rss_bytes / 1024 / 1024,
            snap.p99_latency_us,
            snap.error_count,
        );
    }

    stop.store(true, Ordering::Release);
    for w in workers {
        let _ = w.await;
    }

    // 采集最终快照
    monitor.snapshot((0, 0, 0));

    // 冒烟测试要求：至少完成 50 次操作，且无 PoolLeak
    // 10s 内 4 worker 执行路由解析 + JSON 序列化（微秒级），远超 50 次
    let total_ops = monitor
        .snapshots()
        .last()
        .map(|s| s.ops_completed)
        .unwrap_or(0);
    assert!(
        total_ops >= 50,
        "Soak smoke 应至少完成 50 次操作，实际 {}",
        total_ops
    );

    let regressions = monitor.detect_regressions();
    // 短时测试允许 RSS 微小波动，但不应有 PoolLeak
    let critical: Vec<&SoakRegression> = regressions
        .iter()
        .filter(|r| matches!(r, SoakRegression::PoolLeak { .. }))
        .collect();
    assert!(
        critical.is_empty(),
        "Soak smoke 不应有 PoolLeak：{:?}",
        critical
    );
}
