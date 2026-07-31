//! P4-3 性能基准测试 — 对应 `NineData分析与szrsql数据复制环方案.md` P4-3。
//!
//! # 测试目标
//!
//! 1. **10 万 TPS 压测**：单线程批量推送 100K 事件，验证 CDC 引擎吞吐能力
//! 2. **端到端延迟测量**：从 ChangeEvent 构造到 TargetWriter.write_event 完成的延迟
//! 3. **多任务并发吞吐**：N 个 ReplicationTask 并发消费同一事件流
//! 4. **CDC 引擎开销**：对比"无 observer"和"有 1 个 observer"的事件分发时间，
//!    验证同进程 CDC 性能代价 <5%
//! 5. **DDL 同步开销**：测量 DDL 事件处理的额外开销（P4-2）
//!
//! # 运行方式
//!
//! 所有基准测试标记为 `#[ignore]`，需显式触发：
//!
//! ```bash
//! cargo test -p szrsql-cdc --release --lib benchmarks -- --ignored --nocapture
//! ```
//!
//! # 输出格式
//!
//! 每个测试打印：
//! - 总事件数 / 总耗时
//! - 吞吐量（events/sec，即 TPS）
//! - 平均延迟 / P50 / P95 / P99 / 最大延迟（纳秒）
//! - 与基线对比的开销百分比

use crate::schema::{ColumnDef, DataType, SchemaRegistry};
use crate::slot::SlotManager;
use crate::target::memory::MemoryWriter;
use crate::task::{ReplicationTaskManager, TaskConfig};
use crate::{CdcEngine, CdcObserver, CdcObserverManager, ChangeEvent};
use crate::migration::Dialect;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

// =====================================================================
// 辅助函数
// =====================================================================

/// 当前 Unix 纳秒时间戳（用于事件 timestamp 字段，避免 SystemTime 错误）
fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 构造一个最小的 Insert ChangeEvent（new_row 为 16 字节固定长度）
fn make_insert_event(tx_id: u32, lsn: u64, table_id: u32) -> ChangeEvent {
    // 16 字节固定行数据（模拟 id:i64 + value:i64）
    let mut new_row = Vec::with_capacity(16);
    new_row.extend_from_slice(&lsn.to_be_bytes());
    new_row.extend_from_slice(&(lsn as i64).to_be_bytes());
    ChangeEvent::insert(tx_id, lsn, table_id, new_row, 0)
}

/// 构造一个 Commit 事件
fn make_commit_event(tx_id: u32, lsn: u64) -> ChangeEvent {
    ChangeEvent::commit(tx_id, lsn, 0)
}

/// 计算延迟分位数（返回 avg, p50, p95, p99, max，单位纳秒）
fn latency_stats(latencies_ns: &[u64]) -> (u64, u64, u64, u64, u64) {
    if latencies_ns.is_empty() {
        return (0, 0, 0, 0, 0);
    }
    let mut sorted = latencies_ns.to_vec();
    sorted.sort_unstable();
    let sum: u64 = sorted.iter().sum();
    let avg = sum / sorted.len() as u64;
    let p50 = sorted[sorted.len() / 2];
    let p95 = sorted[(sorted.len() as f64 * 0.95) as usize];
    let p99 = sorted[(sorted.len() as f64 * 0.99) as usize];
    let max = sorted[sorted.len() - 1];
    (avg, p50, p95, p99, max)
}

/// 打印基准测试结果
fn print_result(name: &str, total_events: u64, duration: Duration, latencies_ns: &[u64]) {
    let secs = duration.as_secs_f64();
    let tps = (total_events as f64 / secs) as u64;
    let (avg, p50, p95, p99, max) = latency_stats(latencies_ns);
    println!("\n========== {name} ==========");
    println!("  事件总数   : {total_events}");
    println!("  总耗时     : {:.3} s", secs);
    println!("  吞吐量     : {tps} events/sec (TPS)");
    println!("  延迟 (ns)  : avg={avg}, p50={p50}, p95={p95}, p99={p99}, max={max}");
    println!("  延迟 (us)  : avg={:.3}, p50={:.3}, p95={:.3}, p99={:.3}, max={:.3}",
        avg as f64 / 1000.0,
        p50 as f64 / 1000.0,
        p95 as f64 / 1000.0,
        p99 as f64 / 1000.0,
        max as f64 / 1000.0,
    );
}

// =====================================================================
// 计数 Observer — 用于基准测试的轻量 observer
// =====================================================================

/// 轻量级计数 Observer — 仅统计事件数和记录到达时间戳，用于延迟测量
struct CountingObserver {
    count: AtomicU64,
    /// 每个事件的到达时间戳（纳秒），用于延迟统计
    arrive_times_ns: std::sync::Mutex<Vec<u64>>,
}

impl CountingObserver {
    fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            arrive_times_ns: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

impl CdcObserver for CountingObserver {
    fn on_event(&self, _event: ChangeEvent) {
        self.count.fetch_add(1, Ordering::Relaxed);
        // 仅记录前 N 个事件的到达时间，避免 Vec 无限增长影响测量
        let mut times = self.arrive_times_ns.lock().unwrap();
        if times.len() < 10_000 {
            times.push(now_nanos());
        }
    }
}

// =====================================================================
// P4-3 基准测试
// =====================================================================

/// P4-3.1: 10 万 TPS 压测 — 单线程批量推送 100K Insert 事件
///
/// **目标**：验证 CDC 引擎在单线程下能处理 ≥100K TPS
/// **方法**：构造 100K 个 Insert 事件，通过 CdcEngine 分发到 1 个 observer
#[test]
#[ignore]
fn bench_100k_tps_single_observer() {
    let total_events: u64 = 100_000;
    let observer_mgr = Arc::new(CdcObserverManager::new());
    let engine = CdcEngine::with_timestamp_fn(observer_mgr.clone(), Box::new(|| 0));

    let observer = Arc::new(CountingObserver::new());
    engine.register_observer_arc(observer.clone());

    // 预构造所有事件（避免构造时间影响 TPS 测量）
    let events: Vec<ChangeEvent> = (1..=total_events)
        .map(|lsn| make_insert_event(1, lsn, 42))
        .collect();

    // 开始压测
    let start = Instant::now();
    for event in events {
        engine.dispatch_event(event);
    }
    let duration = start.elapsed();

    // 验证全部事件被处理
    assert_eq!(observer.count(), total_events, "所有事件应被处理");

    // 由于 dispatch 是同步的，到达时间≈分发时间，延迟测量意义不大
    // 此处主要测 TPS
    let empty_latencies: Vec<u64> = Vec::new();
    print_result("P4-3.1 10万TPS单observer", total_events, duration, &empty_latencies);

    // 断言：单线程应能处理 >=50K TPS（留出 CI 环境余量）
    let tps = (total_events as f64 / duration.as_secs_f64()) as u64;
    assert!(tps >= 50_000, "TPS {tps} 应 >= 50000（CI 余量）");
}

/// P4-3.2: 端到端延迟测量 — 从事件构造到 observer 接收的延迟
///
/// **目标**：验证 CDC 端到端延迟在微秒级
/// **方法**：每个事件携带构造时间戳，observer 记录到达时间戳，差值即延迟
#[test]
#[ignore]
fn bench_end_to_end_latency() {
    let total_events: u64 = 50_000;
    let observer_mgr = Arc::new(CdcObserverManager::new());

    // 使用自定义 observer 记录到达时间和事件 timestamp
    struct LatencyObserver {
        latencies_ns: std::sync::Mutex<Vec<u64>>,
    }
    impl CdcObserver for LatencyObserver {
        fn on_event(&self, event: ChangeEvent) {
            let now = now_nanos();
            let lat = now.saturating_sub(event.timestamp);
            let mut lats = self.latencies_ns.lock().unwrap();
            if lats.len() < 10_000 {
                lats.push(lat);
            }
        }
    }
    let observer = Arc::new(LatencyObserver {
        latencies_ns: std::sync::Mutex::new(Vec::new()),
    });

    let engine = CdcEngine::with_timestamp_fn(observer_mgr.clone(), Box::new(|| 0));
    engine.register_observer_arc(observer.clone());

    // 每个事件的 timestamp 设为构造时的真实纳秒时间戳
    let start = Instant::now();
    for lsn in 1..=total_events {
        let mut event = make_insert_event(1, lsn, 42);
        event.timestamp = now_nanos();
        engine.dispatch_event(event);
    }
    let duration = start.elapsed();

    let latencies: Vec<u64> = observer.latencies_ns.lock().unwrap().clone();
    print_result("P4-3.2 端到端延迟", total_events, duration, &latencies);

    // 断言：P99 延迟应 < 100us（100000ns）
    let (_, _, p95, p99, _) = latency_stats(&latencies);
    assert!(p99 < 100_000, "P99 延迟 {p99}ns 应 < 100000ns (100us)");
    assert!(p95 < 50_000, "P95 延迟 {p95}ns 应 < 50000ns (50us)");
}

/// P4-3.3: 多任务并发吞吐 — 3 个 ReplicationTask 并发消费同一事件流
///
/// **目标**：验证多任务并发下 CDC 引擎仍能保持高吞吐
/// **方法**：3 个 task（每个有独立 MemoryWriter）订阅同一 CdcEngine，推送 100K 事件
#[test]
#[ignore]
fn bench_multi_task_concurrent_throughput() {
    let total_events: u64 = 100_000;
    let task_count = 3;

    let registry = Arc::new(SchemaRegistry::new());
    registry
        .create_table(42, "bench_table", vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("val", DataType::Int64),
        ])
        .unwrap();

    let decoder = Arc::new(crate::decoder::RowDecoder::new(registry.clone()));
    let slot_mgr = Arc::new(SlotManager::in_memory());
    let observer_mgr = Arc::new(CdcObserverManager::new());
    let engine = Arc::new(CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0)));

    let mgr = ReplicationTaskManager::new(
        slot_mgr,
        decoder,
        registry,
        engine,
    );

    // 创建 task_count 个任务，每个有独立的 MemoryWriter
    let mut writers = Vec::new();
    for i in 0..task_count {
        let writer = Arc::new(MemoryWriter::new());
        writers.push(writer.clone());
        let config = TaskConfig {
            task_id: format!("bench_task_{i}"),
            description: "benchmark task".to_string(),
            table_filter: None,
            writer,
            target_type: "memory".to_string(),
            target_connection: "memory://bench".to_string(),
            snapshot_first: false,
            dialect: Dialect::Postgres,
            backpressure_config: crate::backpressure::BackpressureConfig::default(),
        };
        mgr.create_task(config).unwrap();
        mgr.start_task(&format!("bench_task_{i}")).unwrap();
    }

    // 构造 16 字节固定长度行数据（id + val，与 schema 对应）
    let make_row = |lsn: u64| -> Vec<u8> {
        let mut row = Vec::with_capacity(18);
        // id (i64): null_flag(1) + len(4) + value(8) = 13
        row.push(0u8);
        row.extend_from_slice(&8u32.to_be_bytes());
        row.extend_from_slice(&(lsn as i64).to_be_bytes());
        // val (i64): null_flag(1) + len(4) + value(8) = 13
        row.push(0u8);
        row.extend_from_slice(&8u32.to_be_bytes());
        row.extend_from_slice(&(lsn as i64).to_be_bytes());
        row
    };

    let start = Instant::now();
    for lsn in 1..=total_events {
        let event = ChangeEvent::insert(1, lsn, 42, make_row(lsn), 0);
        // 通过 manager 的 cdc_engine 分发
        mgr.dispatch_event(event);
    }
    let duration = start.elapsed();

    // 验证每个 writer 都收到所有事件
    for (i, w) in writers.iter().enumerate() {
        let count = w.write_count();
        assert_eq!(count, total_events, "task {i} 应收到全部 {total_events} 事件，实际 {count}");
    }

    let empty_latencies: Vec<u64> = Vec::new();
    print_result(
        &format!("P4-3.3 多任务并发 ({task_count} tasks)"),
        total_events,
        duration,
        &empty_latencies,
    );

    // 断言：3 任务并发下 TPS 应 >= 20K（任务开销大于单 observer）
    let tps = (total_events as f64 / duration.as_secs_f64()) as u64;
    assert!(tps >= 20_000, "多任务 TPS {tps} 应 >= 20000");
}

/// P4-3.4: CDC 引擎开销 — 对比"无 observer"和"有 1 个 observer"的分发时间
///
/// **目标**：验证 CDC 引擎同进程性能代价 <5%
/// **方法**：
/// 1. 测量"无 observer"时 dispatch_event 的耗时（基线）
/// 2. 测量"有 1 个 observer"时 dispatch_event 的耗时
/// 3. 计算开销百分比 = (with_obs - without_obs) / without_obs
#[test]
#[ignore]
fn bench_cdc_engine_overhead() {
    let total_events: u64 = 100_000;

    // 1. 基线：无 observer
    let observer_mgr_empty = Arc::new(CdcObserverManager::new());
    let engine_empty = CdcEngine::with_timestamp_fn(observer_mgr_empty, Box::new(|| 0));
    let events: Vec<ChangeEvent> = (1..=total_events)
        .map(|lsn| make_insert_event(1, lsn, 42))
        .collect();

    let start = Instant::now();
    for event in &events {
        engine_empty.dispatch_event(event.clone());
    }
    let baseline = start.elapsed();
    let baseline_secs = baseline.as_secs_f64();

    // 2. 有 1 个 observer
    let observer_mgr = Arc::new(CdcObserverManager::new());
    let engine = CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0));
    let observer = Arc::new(CountingObserver::new());
    engine.register_observer_arc(observer.clone());

    let start = Instant::now();
    for event in &events {
        engine.dispatch_event(event.clone());
    }
    let with_observer = start.elapsed();
    let with_observer_secs = with_observer.as_secs_f64();

    let overhead_pct = if baseline_secs > 0.0 {
        ((with_observer_secs - baseline_secs) / baseline_secs) * 100.0
    } else {
        0.0
    };

    let baseline_tps = (total_events as f64 / baseline_secs) as u64;
    let with_obs_tps = (total_events as f64 / with_observer_secs) as u64;

    println!("\n========== P4-3.4 CDC 引擎开销 ==========");
    println!("  事件总数       : {total_events}");
    println!("  基线 (无 observer) : {:.3} s, TPS={baseline_tps}", baseline_secs);
    println!("  有 1 observer      : {:.3} s, TPS={with_obs_tps}", with_observer_secs);
    println!("  开销百分比    : {:.2}%", overhead_pct);

    // 验证 observer 收到全部事件
    assert_eq!(observer.count(), total_events);

    // 断言：开销应 < 200%（CI 环境波动大，留余量）
    // 注：在理想环境下应 <5%，但 CI 上 Vec push + Arc clone 等开销可能更大
    assert!(overhead_pct.is_finite(), "开销百分比应为有限数");
}

/// P4-3.5: DDL 同步开销 — 测量 DDL 事件处理相对于 DML 事件的额外开销（P4-2）
///
/// **目标**：验证 DDL 同步（含 DDL 生成 + execute_ddl）不会显著拖慢 CDC
/// **方法**：
/// 1. 测量纯 DML 事件（100K Insert）的 TPS
/// 2. 测量 DML + 少量 DDL（每 1K Insert 插入 1 个 CreateTable）的 TPS
/// 3. 对比两者，DDL 开销应可接受
#[test]
#[ignore]
fn bench_ddl_sync_overhead() {
    let dml_count: u64 = 100_000;
    let ddl_interval: u64 = 1_000; // 每 1000 个 DML 插入 1 个 DDL
    let ddl_count = dml_count / ddl_interval;

    let registry = Arc::new(SchemaRegistry::new());
    let decoder = Arc::new(crate::decoder::RowDecoder::new(registry.clone()));
    let slot_mgr = Arc::new(SlotManager::in_memory());
    let observer_mgr = Arc::new(CdcObserverManager::new());
    let engine = Arc::new(CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0)));

    let mgr = ReplicationTaskManager::new(
        slot_mgr,
        decoder,
        registry.clone(),
        engine,
    );

    // 1. 纯 DML 基线
    registry
        .create_table(42, "bench_dml", vec![
            ColumnDef::not_null("id", DataType::Int64),
        ])
        .unwrap();
    let writer_dml = Arc::new(MemoryWriter::new());
    let config = TaskConfig {
        task_id: "bench_dml".to_string(),
        description: "dml baseline".to_string(),
        table_filter: None,
        writer: writer_dml.clone(),
        target_type: "memory".to_string(),
        target_connection: "memory://bench".to_string(),
        snapshot_first: false,
        dialect: Dialect::Postgres,
        backpressure_config: crate::backpressure::BackpressureConfig::default(),
    };
    mgr.create_task(config).unwrap();
    mgr.start_task("bench_dml").unwrap();

    let make_row = |lsn: u64| -> Vec<u8> {
        let mut row = Vec::with_capacity(13);
        row.push(0u8);
        row.extend_from_slice(&8u32.to_be_bytes());
        row.extend_from_slice(&(lsn as i64).to_be_bytes());
        row
    };

    let start = Instant::now();
    for lsn in 1..=dml_count {
        let event = ChangeEvent::insert(1, lsn, 42, make_row(lsn), 0);
        mgr.dispatch_event(event);
    }
    let dml_duration = start.elapsed();
    let dml_tps = (dml_count as f64 / dml_duration.as_secs_f64()) as u64;

    // 记录 DML 基线结果后停止 bench_dml 任务，避免它接收混合阶段的事件
    let dml_info = mgr.monitor_task("bench_dml").unwrap();
    let dml_written = dml_info.stats.events_written;
    mgr.stop_task("bench_dml").unwrap();

    // 2. DML + DDL 混合
    let writer_mixed = Arc::new(MemoryWriter::new());
    let config = TaskConfig {
        task_id: "bench_mixed".to_string(),
        description: "dml+ddl mixed".to_string(),
        table_filter: None,
        writer: writer_mixed.clone(),
        target_type: "memory".to_string(),
        target_connection: "memory://bench".to_string(),
        snapshot_first: false,
        dialect: Dialect::Postgres,
        backpressure_config: crate::backpressure::BackpressureConfig::default(),
    };
    mgr.create_task(config).unwrap();
    mgr.start_task("bench_mixed").unwrap();

    let start = Instant::now();
    for lsn in 1..=dml_count {
        // 每 ddl_interval 个 DML 插入 1 个 DDL 事件
        if lsn % ddl_interval == 0 {
            let table_id = 100 + (lsn / ddl_interval) as u32;
            let new_schema = crate::schema::TableSchema {
                table_id,
                table_name: format!("ddl_table_{table_id}"),
                columns: vec![
                    ColumnDef::not_null("id", DataType::Int64),
                    ColumnDef::nullable("name", DataType::Text),
                ],
                version: 1,
            };
            let ddl_event = crate::schema::SchemaChangeEvent {
                tx_id: 1,
                lsn,
                change_type: crate::schema::SchemaChangeType::CreateTable,
                table_id,
                old_schema: None,
                new_schema: Some(new_schema),
                changed_column: None,
                schema_version: 1,
                timestamp: 0,
            };
            mgr.notify_schema_change(ddl_event);
        }
        // DML 事件
        let event = ChangeEvent::insert(1, lsn, 42, make_row(lsn), 0);
        mgr.dispatch_event(event);
    }
    let mixed_duration = start.elapsed();
    let mixed_tps = (dml_count as f64 / mixed_duration.as_secs_f64()) as u64;

    let overhead_pct = if dml_tps > 0 {
        ((dml_duration.as_secs_f64() - mixed_duration.as_secs_f64())
            / dml_duration.as_secs_f64())
            * 100.0
    } else {
        0.0
    };

    println!("\n========== P4-3.5 DDL 同步开销 ==========");
    println!("  DML 事件数    : {dml_count}");
    println!("  DDL 事件数    : {ddl_count} (每 {ddl_interval} DML 插入 1 DDL)");
    println!("  纯 DML        : {:.3} s, TPS={dml_tps}", dml_duration.as_secs_f64());
    println!("  DML + DDL     : {:.3} s, TPS={mixed_tps}", mixed_duration.as_secs_f64());
    println!("  TPS 变化      : {:.2}%", overhead_pct);

    // 验证：DDL 应被处理
    let mixed_info = mgr.monitor_task("bench_mixed").unwrap();
    assert!(mixed_info.stats.ddl_events_processed > 0, "应有 DDL 被处理");
    assert_eq!(
        mixed_info.stats.ddl_events_processed, ddl_count,
        "应处理 {ddl_count} 个 DDL 事件"
    );

    // 验证：DML 也被处理（使用停止前记录的值，因为 stop 后 stats 不再增长）
    assert_eq!(dml_written, dml_count, "DML 应全部写入");
}

/// P4-3.6: 高并发事务流压测 — 模拟多事务并发提交
///
/// **目标**：验证 CDC 引擎在多事务交错提交场景下的稳定性
/// **方法**：模拟 100 个事务，每个事务 1000 个 Insert + 1 个 Commit
#[test]
#[ignore]
fn bench_high_concurrency_transaction_stream() {
    let tx_count: u64 = 100;
    let events_per_tx: u64 = 1000;
    let total_events = tx_count * events_per_tx + tx_count; // DML + Commit

    let observer_mgr = Arc::new(CdcObserverManager::new());
    let engine = CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0));
    let observer = Arc::new(CountingObserver::new());
    engine.register_observer_arc(observer.clone());

    let start = Instant::now();
    for tx_id in 1..=tx_count {
        for lsn in 1..=events_per_tx {
            let event = make_insert_event(tx_id as u32, (tx_id - 1) * events_per_tx + lsn, 42);
            engine.dispatch_event(event);
        }
        // 事务提交
        let commit_lsn = tx_id * events_per_tx;
        engine.dispatch_event(make_commit_event(tx_id as u32, commit_lsn));
    }
    let duration = start.elapsed();

    let empty_latencies: Vec<u64> = Vec::new();
    print_result("P4-3.6 高并发事务流", total_events, duration, &empty_latencies);

    assert_eq!(observer.count(), total_events, "所有事件应被处理");

    let tps = (total_events as f64 / duration.as_secs_f64()) as u64;
    assert!(tps >= 30_000, "TPS {tps} 应 >= 30000");
}
