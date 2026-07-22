//! Runtime 性能对比测试（Phase 9.10）
//!
//! ## 测试目标
//!
//! 验证 SZ-Rust tokio runtime 相比 PHP Swoole 的性能特征：
//!
//! 1. **spawn 吞吐量**：tokio::spawn vs Swoole\Coroutine::create
//! 2. **CancellationToken 取消传播延迟**：token.cancel() 到任务响应的延迟
//! 3. **JoinSet 任务管理开销**：管理大量任务的开销
//! 4. **Scheduler tick 延迟**：tokio::time::interval + try_fire_due 的 tick 精度
//! 5. **Queue 消费吞吐量**：InMemoryQueue publish/consume 循环吞吐
//!
//! ## PHP 对比基线
//!
//! | 指标 | PHP Swoole 基线 | Rust 目标 |
//! |------|----------------|-----------|
//! | spawn 10k tasks | ~50ms | <100ms |
//! | cancel 传播延迟 | ~1ms | <1ms |
//! | Scheduler tick 精度 | ±5ms | ±1ms |
//!
//! ## 注意
//!
//! - 性能测试受 CI 环境波动影响，阈值设为宽松值
//! - 不与 PHP 实测对比，仅验证 Rust 侧性能在合理范围
//! - 标记为 `#[ignore]` 默认不运行，通过 `--ignored` 显式触发

#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sz_rust_core::runtime::{
    spawn_with_token, GracefulShutdown, QueueRuntime, QueueRuntimeConfig, SchedulerRuntime,
    SzRuntime, WorkerConfig,
};
use tokio_util::sync::CancellationToken;

/// 辅助函数：测量异步操作的耗时
async fn measure_async<F, Fut, T>(label: &str, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let start = Instant::now();
    let result = f().await;
    let elapsed = start.elapsed();
    println!("[PERF] {}: {:?}", label, elapsed);
    result
}

// ============================================================================
// 组 1：spawn 吞吐量测试
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_spawn_10k_tasks_throughput() {
    let rt = SzRuntime::with_worker_threads(4);
    let counter = Arc::new(AtomicUsize::new(0));

    measure_async("spawn 10k tasks", || async {
        let mut handles = Vec::with_capacity(10_000);
        for _ in 0..10_000 {
            let c = counter.clone();
            handles.push(rt.spawn(async move {
                c.fetch_add(1, Ordering::Relaxed);
            }));
        }
        // 等待所有任务完成
        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert_eq!(counter.load(Ordering::SeqCst), 10_000);
}

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_spawn_with_token_1k_tasks_throughput() {
    let token = CancellationToken::new();
    let counter = Arc::new(AtomicUsize::new(0));

    measure_async("spawn_with_token 1k tasks", || async {
        let mut handles = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            let t = token.clone();
            let c = counter.clone();
            handles.push(spawn_with_token(t, async move {
                c.fetch_add(1, Ordering::Relaxed);
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert_eq!(counter.load(Ordering::SeqCst), 1_000);
}

// ============================================================================
// 组 2：CancellationToken 取消传播延迟
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_cancel_propagation_latency() {
    let token = CancellationToken::new();
    let token_clone = token.clone();
    let completed = Arc::new(AtomicUsize::new(0));
    let completed_clone = completed.clone();

    // spawn 100 个监听 token 的任务
    for _ in 0..100 {
        let t = token.clone();
        let c = completed.clone();
        tokio::spawn(async move {
            t.cancelled().await;
            c.fetch_add(1, Ordering::Relaxed);
        });
    }

    // 测量从 cancel() 到所有任务响应的延迟
    let start = Instant::now();
    token_clone.cancel();

    // 等待所有任务完成
    tokio::time::sleep(Duration::from_millis(50)).await;
    let elapsed = start.elapsed();

    println!("[PERF] cancel propagation for 100 tasks: {:?}", elapsed);
    assert_eq!(completed_clone.load(Ordering::SeqCst), 100);
    // 取消传播应在 50ms 内完成
    assert!(
        elapsed < Duration::from_millis(50),
        "cancel propagation too slow: {:?}",
        elapsed
    );
}

// ============================================================================
// 组 3：JoinSet (GracefulShutdown) 任务管理开销
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_graceful_shutdown_1k_tasks() {
    let mut gs = GracefulShutdown::new();

    measure_async("GracefulShutdown spawn 1k tasks", || async {
        for _ in 0..1_000 {
            let token = gs.token();
            gs.spawn(async move {
                token.cancelled().await;
            });
        }
    })
    .await;

    assert_eq!(gs.len(), 1_000);

    let (success, aborted) = measure_async("GracefulShutdown shutdown 1k tasks", || async {
        gs.shutdown(Duration::from_secs(1)).await
    })
    .await;

    assert!(success);
    assert_eq!(aborted, 0);
}

// ============================================================================
// 组 4：Scheduler tick 精度
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_scheduler_tick_latency() {
    use sz_rust_core::runtime::scheduler::SchedulerRuntimeConfig;

    let config = SchedulerRuntimeConfig::new(100);
    let sr = SchedulerRuntime::new(config);

    // 测量 10 次 tick 的总耗时
    let tick_count = 10;
    let expected_total = Duration::from_millis(100 * tick_count); // 100ms per tick

    let start = Instant::now();
    for _ in 0..tick_count {
        tokio::time::sleep(Duration::from_millis(100)).await;
        sr.try_fire_due();
    }
    let elapsed = start.elapsed();

    println!(
        "[PERF] scheduler {} ticks: {:?} (expected ~{:?})",
        tick_count, elapsed, expected_total
    );

    // 误差应在 ±20% 以内
    let tolerance = expected_total / 5;
    let lower = expected_total - tolerance;
    let upper = expected_total + tolerance;
    assert!(
        elapsed >= lower && elapsed <= upper,
        "tick latency out of tolerance: {:?} not in [{:?}, {:?}]",
        elapsed,
        lower,
        upper
    );
}

// ============================================================================
// 组 5：Queue 消费吞吐量
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_queue_publish_consume_throughput() {
    use sz_orm_queue::{InMemoryQueue, MessageQueue};

    let queue: Arc<dyn MessageQueue> = Arc::new(InMemoryQueue::new());
    let message_count = 1_000usize;

    // 测量 publish 吞吐
    measure_async("publish 1k messages", || async {
        for i in 0..message_count {
            let payload = format!("msg-{}", i);
            queue
                .publish("perf-test", payload.as_bytes())
                .await
                .unwrap();
        }
    })
    .await;

    // 测量 consume 吞吐
    let consumed = Arc::new(AtomicUsize::new(0));
    let consumed_clone = consumed.clone();
    measure_async("consume 1k messages", || async {
        loop {
            let msg = queue.consume("perf-test").await.unwrap();
            if msg.is_none() {
                break;
            }
            consumed_clone.fetch_add(1, Ordering::Relaxed);
        }
    })
    .await;

    assert_eq!(consumed.load(Ordering::SeqCst), message_count);
}

// ============================================================================
// 组 6：WorkerConfig 边界性能
// ============================================================================

#[tokio::test]
async fn perf_worker_config_build_benchmark() {
    let iterations = 10_000;
    let start = Instant::now();
    for _ in 0..iterations {
        let _config = WorkerConfig::new()
            .with_worker_num(8)
            .with_reactor_num(4)
            .with_task_worker_num(2);
    }
    let elapsed = start.elapsed();
    let per_op = elapsed / iterations as u32;
    println!(
        "[PERF] WorkerConfig build x{}: {:?} (per-op: {:?})",
        iterations, elapsed, per_op
    );
    // WorkerConfig 构建应在微秒级
    assert!(
        per_op < Duration::from_micros(100),
        "WorkerConfig build too slow: {:?} per op",
        per_op
    );
}

// ============================================================================
// 组 7：CancellationToken clone 性能
// ============================================================================

#[tokio::test]
async fn perf_token_clone_benchmark() {
    let token = CancellationToken::new();
    let iterations = 100_000;
    let start = Instant::now();
    for _ in 0..iterations {
        let _clone = token.clone();
    }
    let elapsed = start.elapsed();
    let per_op = elapsed / iterations as u32;
    println!(
        "[PERF] CancellationToken clone x{}: {:?} (per-op: {:?})",
        iterations, elapsed, per_op
    );
    // clone 应在纳秒级
    assert!(
        per_op < Duration::from_micros(10),
        "token clone too slow: {:?} per op",
        per_op
    );
}

// ============================================================================
// 组 8：SzRuntime 创建/销毁开销
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_runtime_create_destroy() {
    let iterations = 10;
    let start = Instant::now();
    for _ in 0..iterations {
        let rt = SzRuntime::with_worker_threads(1);
        // 立即关闭
        rt.shutdown_timeout(Duration::from_millis(10));
    }
    let elapsed = start.elapsed();
    let per_op = elapsed / iterations as u32;
    println!(
        "[PERF] SzRuntime create+destroy x{}: {:?} (per-op: {:?})",
        iterations, elapsed, per_op
    );
    // runtime 创建+销毁应在 100ms 以内
    assert!(
        per_op < Duration::from_millis(100),
        "runtime create/destroy too slow: {:?} per op",
        per_op
    );
}

// ============================================================================
// 组 9：QueueRuntime 消费循环性能
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_queue_runtime_consumer_loop() {
    use sz_orm_queue::{InMemoryQueue, MessageQueue};

    let queue: Arc<dyn MessageQueue> = Arc::new(InMemoryQueue::new());

    // 预发布消息
    for i in 0..100 {
        let payload = format!("msg-{}", i);
        queue.publish("perf", payload.as_bytes()).await.unwrap();
    }

    let config = QueueRuntimeConfig::new("perf").with_poll_interval(1);
    let runtime = QueueRuntime::new(config, queue.clone());

    struct CountingConsumer {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl sz_rust_core::runtime::QueueConsumer for CountingConsumer {
        async fn handle(
            &self,
            _message: &sz_orm_queue::Message,
        ) -> Result<(), sz_rust_core::runtime::queue::QueueConsumerError> {
            self.counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    let counter = Arc::new(AtomicUsize::new(0));
    let consumer = Arc::new(CountingConsumer {
        counter: counter.clone(),
    });

    let token = CancellationToken::new();
    let token_clone = token.clone();

    let handle = runtime.start(consumer, token);

    // 等待消费完成
    tokio::time::sleep(Duration::from_millis(200)).await;
    token_clone.cancel();
    let _ = handle.await;

    println!(
        "[PERF] QueueRuntime consumed: {}",
        counter.load(Ordering::SeqCst)
    );
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

// ============================================================================
// 组 10：综合场景 — spawn + cancel + join
// ============================================================================

#[tokio::test]
#[ignore = "性能测试默认不运行，使用 --ignored 触发"]
async fn perf_integrated_scenario() {
    let rt = SzRuntime::with_worker_threads(2);
    let token = rt.shutdown_token();
    let counter = Arc::new(AtomicUsize::new(0));

    // spawn 100 个任务，每个任务循环 100 次
    let mut handles = Vec::with_capacity(100);
    for _ in 0..100 {
        let t = token.clone();
        let c = counter.clone();
        handles.push(rt.spawn(async move {
            for _ in 0..100 {
                if t.is_cancelled() {
                    return;
                }
                c.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
            }
        }));
    }

    // 等待一段时间让任务执行
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 触发关闭
    let start = Instant::now();
    rt.shutdown_timeout(Duration::from_millis(500));
    let elapsed = start.elapsed();

    println!(
        "[PERF] integrated scenario: counter={}, shutdown={:?}",
        counter.load(Ordering::SeqCst),
        elapsed
    );

    // 验证任务确实执行了
    assert!(counter.load(Ordering::SeqCst) > 0);
}
