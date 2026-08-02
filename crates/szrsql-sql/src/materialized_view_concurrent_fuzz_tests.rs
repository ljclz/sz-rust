//! Phase 6.16 集成测试 — 物化视图 + CDC 并发 fuzz。
//!
//! 覆盖类别：
//! - 并发 fuzz 基础（2 条）：8 线程并发写入源表 + CDC 事件流 → 增量刷新 → 全表对比
//! - 并发 fuzz 不同种子（2 条）：不同随机种子下增量与全量一致
//! - 单线程 fuzz 基线（1 条）：单线程大量 DML → 增量 vs 全量一致（验证 fuzz 逻辑正确性）
//! - 线程安全（1 条）：CdcFeed 在 Arc<Mutex> 下跨线程共享
//! - 高并发压力（1 条）：16 线程 × 2000 操作 → 增量 vs 全量一致
//!
//! 共 7 个测试用例。
//!
//! # 设计
//!
//! - **源表状态**：`Arc<Mutex<HashMap<i64, String>>>` 表示 users 表的当前状态
//! - **CDC 事件流**：`Arc<Mutex<CdcFeed>>` 收集所有线程的 CDC 事件
//! - **线程隔离**：每个线程拥有独立的键范围（避免跨线程主键冲突）
//! - **DML 操作**：INSERT（UPSERT）/ UPDATE / DELETE 随机选择
//! - **对比方式**：增量刷新结果 vs 全量重建结果（从源表状态直接构建）
//! - **确定性**：使用 xorshift64 PRNG + 固定种子，确保测试可重现

use super::executor::{Executor, TableStorage};
use super::materialized_view::{CdcEvent, CdcFeed, MaterializedViewStore};
use crate::ast::TableName;
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use std::collections::HashMap;
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带 `users` 表的 catalog（id INT PK, name TEXT）
fn make_catalog_with_users() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE users (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();
    catalog
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// 创建并注册物化视图 `mv`（SELECT id, name FROM users）
fn setup_materialized_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv AS SELECT id, name FROM users",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建带主键的物化视图存储（id 为主键，索引 0）
fn make_mv_store_with_pk() -> MaterializedViewStore {
    MaterializedViewStore::new_with_pk(
        "mv",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        vec![0],
    )
}

/// 收集活跃行（按主键排序便于对比）
fn collect_active_rows_sorted(store: &MaterializedViewStore) -> Vec<Vec<Value>> {
    let mut rows: Vec<Vec<Value>> = store.storage.scan_iter().collect();
    rows.sort_by(|a, b| {
        let a_id = a.first().and_then(|v| match v {
            Value::Int64(i) => Some(*i),
            _ => None,
        });
        let b_id = b.first().and_then(|v| match v {
            Value::Int64(i) => Some(*i),
            _ => None,
        });
        a_id.cmp(&b_id)
    });
    rows
}

/// 从源表状态构建全量 MV 存储（用于对比）
fn build_full_store_from_source(source: &HashMap<i64, String>) -> MaterializedViewStore {
    let mut store = make_mv_store_with_pk();
    // 按 id 排序插入（确保确定性）
    let mut keys: Vec<i64> = source.keys().copied().collect();
    keys.sort();
    for id in keys {
        let name = &source[&id];
        store.upsert_row(vec![Value::Int64(id), Value::Text(name.clone())]);
    }
    store
}

// =====================================================================
//  xorshift64 PRNG（确定性随机数生成器）
// =====================================================================

/// xorshift64 PRNG — 确定性伪随机数生成器
///
/// 用于 fuzz 测试的可重现随机性。种子相同 → 序列相同。
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// 创建新 PRNG（种子为 0 时使用默认种子）
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    /// 生成下一个 u64
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// 生成 [0, max) 范围内的 u64
    fn next_range(&mut self, max: u64) -> u64 {
        if max == 0 {
            return 0;
        }
        self.next_u64() % max
    }
}

// =====================================================================
//  并发 fuzz 核心逻辑
// =====================================================================

/// 并发 fuzz 测试核心：多线程并发写入源表 + CDC 事件流 → 增量刷新 → 全表对比
///
/// # 参数
/// - `num_threads`：并发线程数
/// - `ops_per_thread`：每线程操作数
/// - `keys_per_thread`：每线程键范围大小
/// - `seed`：随机种子（确定性）
fn run_concurrent_fuzz(num_threads: usize, ops_per_thread: usize, keys_per_thread: i64, seed: u64) {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);

    // 源表状态（共享）
    let source_state: Arc<Mutex<HashMap<i64, String>>> = Arc::new(Mutex::new(HashMap::new()));
    // CDC 事件流（共享）
    let cdc_feed: Arc<Mutex<CdcFeed>> = Arc::new(Mutex::new(CdcFeed::new()));

    // 启动线程
    let mut handles = Vec::new();
    for thread_id in 0..num_threads {
        let state = Arc::clone(&source_state);
        let feed = Arc::clone(&cdc_feed);
        let thread_seed = seed.wrapping_add((thread_id as u64).wrapping_mul(0x9E3779B97F4A7C15));
        let key_start = thread_id as i64 * keys_per_thread;

        let handle = std::thread::spawn(move || {
            let mut rng = XorShift64::new(thread_seed);
            for _ in 0..ops_per_thread {
                let op_type = rng.next_range(100);
                let key = key_start + (rng.next_range(keys_per_thread as u64) as i64);
                let mut state = state.lock();
                let mut feed = feed.lock();

                if op_type < 50 {
                    // 50% INSERT（UPSERT 语义：新 key → INSERT 事件，已存在 → UPDATE 事件）
                    let name = format!("t{}_k{}_v{}", thread_id, key, rng.next_range(10000));
                    match state.entry(key) {
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            // Key 已存在 → UPDATE 事件（CDC 语义：避免 MV 存储重复追加）
                            e.insert(name.clone());
                            feed.push_update(
                                "users",
                                vec![Value::Int64(key)],
                                vec![Value::Int64(key), Value::Text(name)],
                            );
                        }
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // 新 key → INSERT 事件
                            e.insert(name.clone());
                            feed.push_insert("users", vec![Value::Int64(key), Value::Text(name)]);
                        }
                    }
                } else if op_type < 80 {
                    // 30% UPDATE（仅当 key 存在时）
                    if let std::collections::hash_map::Entry::Occupied(mut e) = state.entry(key) {
                        let name =
                            format!("upd_t{}_k{}_v{}", thread_id, key, rng.next_range(10000));
                        e.insert(name.clone());
                        feed.push_update(
                            "users",
                            vec![Value::Int64(key)],
                            vec![Value::Int64(key), Value::Text(name)],
                        );
                    }
                } else {
                    // 20% DELETE（仅当 key 存在时）
                    if state.remove(&key).is_some() {
                        feed.push_delete("users", vec![Value::Int64(key)]);
                    }
                }
            }
        });
        handles.push(handle);
    }

    // 等待所有线程完成
    for handle in handles {
        handle.join().expect("thread panicked");
    }

    // 取出源表状态和 CDC 事件流
    let source = Arc::try_unwrap(source_state)
        .expect("all threads done")
        .into_inner();
    let mut feed = Arc::try_unwrap(cdc_feed)
        .expect("all threads done")
        .into_inner();

    // 增量刷新：从 CDC 事件流构建 MV
    let mut incr_store = make_mv_store_with_pk();
    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");

    // 分批刷新（SIMPLE 模式支持 INSERT/UPDATE/DELETE）
    let events = feed.drain();
    let batch_size = 1000;
    let mut batch_feed = CdcFeed::new();
    let mut event_count = 0;

    for event in events {
        match &event {
            CdcEvent::Insert { row, .. } => {
                batch_feed.push_insert("users", row.clone());
            }
            CdcEvent::Update { pk, row, .. } => {
                batch_feed.push_update("users", pk.clone(), row.clone());
            }
            CdcEvent::Delete { pk, .. } => {
                batch_feed.push_delete("users", pk.clone());
            }
        }
        event_count += 1;

        if event_count % batch_size == 0 {
            executor
                .refresh_materialized_view_simple(
                    &view_name,
                    &catalog,
                    &mut incr_store,
                    &mut batch_feed,
                    &source_table,
                    event_count as i64,
                )
                .unwrap();
            batch_feed = CdcFeed::new();
        }
    }

    // 刷新剩余事件
    if !batch_feed.is_empty() {
        executor
            .refresh_materialized_view_simple(
                &view_name,
                &catalog,
                &mut incr_store,
                &mut batch_feed,
                &source_table,
                event_count as i64,
            )
            .unwrap();
    }

    // 全量刷新：从源表状态直接构建
    let full_store = build_full_store_from_source(&source);

    // 对比增量 vs 全量
    let incr_rows = collect_active_rows_sorted(&incr_store);
    let full_rows = collect_active_rows_sorted(&full_store);

    assert_eq!(
        incr_rows.len(),
        full_rows.len(),
        "row count mismatch: incr={} vs full={} (source has {} keys)",
        incr_rows.len(),
        full_rows.len(),
        source.len()
    );

    for (i, (incr_row, full_row)) in incr_rows.iter().zip(full_rows.iter()).enumerate() {
        assert_eq!(
            incr_row, full_row,
            "row {i} mismatch: incr={incr_row:?} vs full={full_row:?}"
        );
    }
}

// =====================================================================
//  并发 fuzz 测试（2 条）
// =====================================================================

#[test]
fn concurrent_fuzz_8_threads_basic() {
    // 8 线程 × 500 操作 × 100 键/线程，种子 42
    run_concurrent_fuzz(8, 500, 100, 42);
}

#[test]
fn concurrent_fuzz_8_threads_different_seed() {
    // 8 线程 × 500 操作 × 100 键/线程，种子 12345
    run_concurrent_fuzz(8, 500, 100, 12345);
}

// =====================================================================
//  不同种子测试（2 条）
// =====================================================================

#[test]
fn concurrent_fuzz_seed_999() {
    // 4 线程 × 1000 操作 × 50 键/线程，种子 999
    run_concurrent_fuzz(4, 1000, 50, 999);
}

#[test]
fn concurrent_fuzz_seed_0_default() {
    // 4 线程 × 1000 操作 × 50 键/线程，种子 0（使用默认种子）
    run_concurrent_fuzz(4, 1000, 50, 0);
}

// =====================================================================
//  单线程 fuzz 基线（1 条）
// =====================================================================

#[test]
fn single_thread_fuzz_baseline() {
    // 单线程 × 5000 操作 × 200 键，验证 fuzz 逻辑正确性
    // 排除并发因素，验证 CDC + 增量刷新逻辑本身正确
    run_concurrent_fuzz(1, 5000, 200, 777);
}

// =====================================================================
//  线程安全测试（1 条）
// =====================================================================

#[test]
fn cdc_feed_thread_safe_under_arc_mutex() {
    // 验证 CdcFeed 在 Arc<Mutex> 下可跨线程安全共享
    let feed: Arc<Mutex<CdcFeed>> = Arc::new(Mutex::new(CdcFeed::new()));
    let mut handles = Vec::new();

    for i in 0..8 {
        let feed = Arc::clone(&feed);
        let handle = std::thread::spawn(move || {
            let mut feed = feed.lock();
            feed.push_insert(
                "users",
                vec![Value::Int64(i), Value::Text(format!("user{i}"))],
            );
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    let feed = feed.lock();
    assert_eq!(feed.len(), 8);

    // 验证所有事件都是 INSERT
    let insert_count = feed
        .peek()
        .iter()
        .filter(|e| matches!(e, CdcEvent::Insert { .. }))
        .count();
    assert_eq!(insert_count, 8);
}

// =====================================================================
//  高并发压力测试（1 条）
// =====================================================================

#[test]
fn concurrent_fuzz_16_threads_high_pressure() {
    // 16 线程 × 2000 操作 × 50 键/线程，种子 2024
    // 总计 32000 操作，验证高并发下增量与全量一致
    run_concurrent_fuzz(16, 2000, 50, 2024);
}
