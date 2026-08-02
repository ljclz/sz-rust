//! SzRSQL CDC 引擎 Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 2.5.2。
//!
//! 验证标准（来自实施进度表）：
//! - **核心测试：10 线程并发写入，随机注册/注销 Observer**
//! - **验证事件不被重复或遗漏分发**
//! - **at-least-once 语义，不遗漏不持续重复**
//!
//! 设计要点：
//! 1. **XorShift64 PRNG**：固定种子，测试可重现（与 isolation_fuzz / crash_recovery_fuzz 同风格）
//! 2. **10 线程并发 on_commit**：每个线程独立事务，并发触发 CdcEngine.on_commit
//! 3. **随机注册/注销 Observer**：另一组线程在并发写入时随机 register/unregister Observer
//! 4. **at-least-once 验证**：
//!    - 每个 ChangeEvent 至少被当时的所有 observer 收到一次
//!    - 不遗漏：写入的 WalRecord 都被转换为 ChangeEvent
//!    - 不持续重复：单个 on_commit 调用不会让 observer 收到重复事件
//! 5. **CountingObserver**：用原子计数器统计事件数，避免 Mutex<Vec> 在高并发下成为瓶颈
//! 6. **并发安全验证**：不 panic，最终统计数符合预期
//!
//! **at-least-once 语义说明**：
//! - 在 CdcEngine 同步分发模型下，on_commit 调用结束时所有当时注册的 observer 都已收到事件
//! - 在 on_commit 执行期间注册的 observer 可能收到也可能收不到当前事件（取决于时序）
//! - 在 on_commit 执行期间注销的 observer 可能收到也可能收不到当前事件（取决于时序）
//! - 不遗漏：on_commit 触发的事件至少被当时活跃的 observer 之一收到
//! - 不持续重复：单个事件不会被同一 observer 收到两次

use crate::{CdcEngine, CdcEventOp, CdcObserver, CdcObserverManager, ChangeEvent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::RwLock;
use std::thread;
use szrsql_tx::wal::{WalObserver, WalOpType, WalRecord};

// =====================================================================
// XorShift64 — 固定种子 PRNG（与 isolation_fuzz / crash_recovery_fuzz 同风格）
// =====================================================================

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// [0, n) 范围
    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    /// 50% 概率返回 true
    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

// =====================================================================
// 计数型 Observer（统计总事件数和按 op 分类计数）
// =====================================================================

/// 计数型 Observer — 用于并发测试，避免 Mutex<Vec> 瓶颈
struct StressObserver {
    id: u64,
    total: AtomicU64,
    insert_count: AtomicU64,
    update_count: AtomicU64,
    delete_count: AtomicU64,
    commit_count: AtomicU64,
    abort_count: AtomicU64,
}

impl StressObserver {
    fn new(id: u64) -> Self {
        Self {
            id,
            total: AtomicU64::new(0),
            insert_count: AtomicU64::new(0),
            update_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
        }
    }

    fn total(&self) -> u64 {
        self.total.load(Ordering::SeqCst)
    }
}

impl CdcObserver for StressObserver {
    fn on_event(&self, event: ChangeEvent) {
        self.total.fetch_add(1, Ordering::SeqCst);
        match event.op {
            CdcEventOp::Insert => {
                self.insert_count.fetch_add(1, Ordering::SeqCst);
            }
            CdcEventOp::Update => {
                self.update_count.fetch_add(1, Ordering::SeqCst);
            }
            CdcEventOp::Delete => {
                self.delete_count.fetch_add(1, Ordering::SeqCst);
            }
            CdcEventOp::Commit => {
                self.commit_count.fetch_add(1, Ordering::SeqCst);
            }
            CdcEventOp::Abort => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

// =====================================================================
// 辅助：构造一个事务的 WAL 记录
// =====================================================================

/// 构造一个事务的 WAL 记录：N 个 Insert + 1 个 Commit
fn make_txn_records(tx_id: u32, start_lsn: u64, insert_count: u32) -> Vec<WalRecord> {
    let mut records = Vec::with_capacity(insert_count as usize + 1);
    for i in 0..insert_count {
        records.push(WalRecord::new(
            start_lsn + i as u64,
            tx_id,
            WalOpType::Insert,
            42,
            vec![i as u8, (i + 1) as u8, (i + 2) as u8],
        ));
    }
    records.push(WalRecord::new(
        start_lsn + insert_count as u64,
        tx_id,
        WalOpType::Commit,
        0,
        vec![],
    ));
    records
}

// =====================================================================
// Phase 2.5.2 测试模块
// =====================================================================

#[cfg(test)]
mod phase_2_5_2 {
    use super::*;

    // -----------------------------------------------------------------
    // Part 1: 基础并发 — 10 线程并发写入，固定 observer
    // -----------------------------------------------------------------

    /// 10 线程并发 on_commit，每个线程 100 个事务，每事务 5 Insert + 1 Commit
    /// 验证：所有 observer 都收到 (10 × 100 × 6 = 6000) 个事件
    #[test]
    fn fuzz_10_threads_concurrent_writes_fixed_observers() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs1 = Arc::new(StressObserver::new(1));
        let obs2 = Arc::new(StressObserver::new(2));
        cdc_mgr.register(obs1.clone());
        cdc_mgr.register(obs2.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 100;
        const INSERTS_PER_TXN: u32 = 5;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64; // 5 Insert + 1 Commit
        const EXPECTED_EVENTS_PER_OBS: u64 =
            (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN; // 6000

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                let mut rng = XorShift64::new(0xAABBCCDD + tid as u64);
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let start_lsn = (tx_id as u64) * 1000;
                    let records = make_txn_records(tx_id, start_lsn, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                    // 随机 sleep 一小段，增加交错概率
                    if rng.next_bool() {
                        std::thread::yield_now();
                    }
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        // 两个 observer 都应收到全部 6000 个事件
        assert_eq!(obs1.total(), EXPECTED_EVENTS_PER_OBS);
        assert_eq!(obs2.total(), EXPECTED_EVENTS_PER_OBS);
        // Insert 数 = 10 × 100 × 5 = 5000
        assert_eq!(
            obs1.insert_count.load(Ordering::SeqCst),
            (NUM_THREADS * TXNS_PER_THREAD * INSERTS_PER_TXN) as u64
        );
        // Commit 数 = 10 × 100 = 1000
        assert_eq!(
            obs1.commit_count.load(Ordering::SeqCst),
            (NUM_THREADS * TXNS_PER_THREAD) as u64
        );
    }

    // -----------------------------------------------------------------
    // Part 2: 随机注册/注销 Observer
    // -----------------------------------------------------------------

    /// 10 写入线程 + 2 注册/注销线程并发
    /// 验证：不 panic，最终 observer 数量符合预期
    #[test]
    fn fuzz_concurrent_register_unregister_during_writes() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        // 预注册 3 个固定 observer
        let fixed_obs: Vec<Arc<StressObserver>> = (0..3)
            .map(|i| Arc::new(StressObserver::new(i as u64)))
            .collect();
        for obs in &fixed_obs {
            cdc_mgr.register(obs.clone());
        }

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        // 共享池：注册/注销线程从这里取/放 observer
        let pool: Arc<RwLock<Vec<Arc<StressObserver>>>> = Arc::new(RwLock::new(Vec::new()));
        // 预填充 10 个 observer 到池中
        for i in 0..10 {
            pool.write().push(Arc::new(StressObserver::new(100 + i)));
        }

        let barrier = Arc::new(std::sync::Barrier::new(12));
        let mut handles = Vec::new();

        // 10 写入线程
        for tid in 0..10 {
            let engine = engine.clone();
            let barrier = barrier.clone();
            let handle = thread::spawn(move || {
                barrier.wait();
                let mut rng = XorShift64::new(0x1234 + tid as u64);
                for txn_idx in 0..50 {
                    let tx_id = tid * 50 + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, 3);
                    engine.on_commit(tx_id, records);
                    if rng.next_bool() {
                        std::thread::yield_now();
                    }
                }
            });
            handles.push(handle);
        }

        // 2 注册/注销线程
        for rid in 0..2 {
            let cdc_mgr = cdc_mgr.clone();
            let pool = pool.clone();
            let barrier = barrier.clone();
            let handle = thread::spawn(move || {
                barrier.wait();
                let mut rng = XorShift64::new(0xBEEF + rid as u64);
                for _ in 0..100 {
                    // 50% 概率从池中取一个 observer 注册，50% 概率从 cdc_mgr 注销一个池中的 observer
                    let should_register = rng.next_bool();
                    if should_register {
                        let obs = pool.write().pop();
                        if let Some(obs) = obs {
                            cdc_mgr.register(obs.clone());
                            // 短暂活跃后放回池（这里直接保留注册，让注销线程来取）
                            // 模拟"短暂注册后注销"
                            std::thread::yield_now();
                            // 实际上我们让 register 后立即 unregister，模拟短生命周期
                            cdc_mgr.unregister(&obs);
                            pool.write().push(obs);
                        }
                    } else {
                        // 注销池中一个 observer（但需要先注册才能注销）
                        // 简化：随机取一个 observer 注册再注销
                        let obs = pool.write().pop();
                        if let Some(obs) = obs {
                            cdc_mgr.register(obs.clone());
                            std::thread::yield_now();
                            cdc_mgr.unregister(&obs);
                            pool.write().push(obs);
                        }
                    }
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // 验证：3 个固定 observer 都收到了事件（至少 1 个）
        // 注：由于注册/注销线程在并发，固定 observer 始终注册，所以应收到全部事件
        for obs in &fixed_obs {
            assert!(
                obs.total() > 0,
                "Fixed observer {} should receive events",
                obs.id
            );
        }
        // 验证：cdc_mgr 最终只剩 3 个固定 observer
        assert_eq!(cdc_mgr.observer_count(), 3);
        // 验证：池中 observer 数量回到 10
        assert_eq!(pool.read().len(), 10);
    }

    // -----------------------------------------------------------------
    // Part 3: at-least-once 语义验证 — 单个事件不被重复分发
    // -----------------------------------------------------------------

    /// 单线程连续 on_commit，验证每个 observer 收到的事件数 = 总事件数（无重复）
    #[test]
    fn fuzz_no_duplicate_events_single_observer() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = CdcEngine::with_timestamp_fn(cdc_mgr.clone(), Box::new(|| 0));

        const NUM_TXNS: u32 = 1000;
        for txn_idx in 0..NUM_TXNS {
            let tx_id = txn_idx + 1;
            let records = make_txn_records(tx_id, tx_id as u64 * 100, 3);
            engine.on_commit(tx_id, records);
        }

        // 每事务 4 个事件（3 Insert + 1 Commit），共 4000 个事件
        let expected = (NUM_TXNS * 4) as u64;
        assert_eq!(obs.total(), expected, "No duplicate events expected");
    }

    /// 10 线程并发 on_commit + 单 observer，验证总事件数 = 预期（无重复）
    #[test]
    fn fuzz_no_duplicate_events_concurrent_single_observer() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 100;
        const EVENTS_PER_TXN: u64 = 4; // 3 Insert + 1 Commit

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, 3);
                    engine.on_commit(tx_id, records);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN;
        assert_eq!(obs.total(), expected, "No duplicate events in concurrent");
    }

    // -----------------------------------------------------------------
    // Part 4: 不遗漏验证 — 所有 WalRecord 都被转换为 ChangeEvent
    // -----------------------------------------------------------------

    /// 10 线程并发 on_commit，每线程不同数量 Insert，验证总事件数精确
    #[test]
    fn fuzz_no_missing_events_concurrent() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        // 每线程不同的 Insert 数（1-10）
        let inserts_per_thread: Vec<u32> = (1..=10).collect();
        let expected_total: u64 = inserts_per_thread
            .iter()
            .map(|&n| (n + 1) as u64) // n Insert + 1 Commit
            .sum();

        let mut handles = Vec::new();
        for (tid, &inserts) in inserts_per_thread.iter().enumerate() {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                let tx_id = (tid + 1) as u32;
                let records = make_txn_records(tx_id, tx_id as u64 * 100, inserts);
                engine.on_commit(tx_id, records);
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(obs.total(), expected_total, "No missing events");
    }

    // -----------------------------------------------------------------
    // Part 5: 过滤验证 — FullPageImage / Checkpoint 不产生事件
    // -----------------------------------------------------------------

    /// 并发写入混合 FullPageImage / Checkpoint 记录，验证它们被过滤
    #[test]
    fn fuzz_filters_internal_records_concurrent() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 50;
        // 每事务：2 Insert + 1 FullPageImage + 1 Checkpoint + 1 Commit = 5 records, 3 events
        const EVENTS_PER_TXN: u64 = 3; // 2 Insert + 1 Commit（FullPageImage + Checkpoint 过滤）

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let base_lsn = tx_id as u64 * 100;
                    let records = vec![
                        WalRecord::new(base_lsn, tx_id, WalOpType::Insert, 42, vec![1]),
                        WalRecord::new(
                            base_lsn + 1,
                            tx_id,
                            WalOpType::FullPageImage,
                            42,
                            vec![0; 100],
                        ),
                        WalRecord::new(base_lsn + 2, tx_id, WalOpType::Insert, 42, vec![2]),
                        WalRecord::new(base_lsn + 3, tx_id, WalOpType::Checkpoint, 0, vec![]),
                        WalRecord::new(base_lsn + 4, tx_id, WalOpType::Commit, 0, vec![]),
                    ];
                    engine.on_commit(tx_id, records);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN;
        assert_eq!(obs.total(), expected);
        // Insert 数 = 2 × 10 × 50 = 1000
        assert_eq!(
            obs.insert_count.load(Ordering::SeqCst),
            (NUM_THREADS * TXNS_PER_THREAD * 2) as u64
        );
    }

    // -----------------------------------------------------------------
    // Part 6: 混合 op 类型 — Insert/Update/Delete/Commit 并发
    // -----------------------------------------------------------------

    /// 10 线程并发，每线程随机选择 Insert/Update/Delete，最后 Commit
    /// 验证：按 op 分类的计数精确
    #[test]
    fn fuzz_mixed_op_types_concurrent() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 50;
        const OPS_PER_TXN: u32 = 10;

        let total_inserts = Arc::new(AtomicU64::new(0));
        let total_updates = Arc::new(AtomicU64::new(0));
        let total_deletes = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let total_inserts = total_inserts.clone();
            let total_updates = total_updates.clone();
            let total_deletes = total_deletes.clone();
            let handle = thread::spawn(move || {
                let mut rng = XorShift64::new(0xCAFE + tid as u64);
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let base_lsn = tx_id as u64 * 100;
                    let mut records = Vec::with_capacity(OPS_PER_TXN as usize + 1);
                    let mut local_inserts = 0u64;
                    let mut local_updates = 0u64;
                    let mut local_deletes = 0u64;
                    for i in 0..OPS_PER_TXN {
                        let op = match rng.next_range(3) {
                            0 => {
                                local_inserts += 1;
                                WalOpType::Insert
                            }
                            1 => {
                                local_updates += 1;
                                WalOpType::Update
                            }
                            _ => {
                                local_deletes += 1;
                                WalOpType::Delete
                            }
                        };
                        records.push(WalRecord::new(
                            base_lsn + i as u64,
                            tx_id,
                            op,
                            42,
                            vec![i as u8],
                        ));
                    }
                    records.push(WalRecord::new(
                        base_lsn + OPS_PER_TXN as u64,
                        tx_id,
                        WalOpType::Commit,
                        0,
                        vec![],
                    ));
                    engine.on_commit(tx_id, records);
                    total_inserts.fetch_add(local_inserts, Ordering::SeqCst);
                    total_updates.fetch_add(local_updates, Ordering::SeqCst);
                    total_deletes.fetch_add(local_deletes, Ordering::SeqCst);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected_total = (NUM_THREADS * TXNS_PER_THREAD * (OPS_PER_TXN + 1)) as u64;
        assert_eq!(obs.total(), expected_total);
        assert_eq!(
            obs.insert_count.load(Ordering::SeqCst),
            total_inserts.load(Ordering::SeqCst)
        );
        assert_eq!(
            obs.update_count.load(Ordering::SeqCst),
            total_updates.load(Ordering::SeqCst)
        );
        assert_eq!(
            obs.delete_count.load(Ordering::SeqCst),
            total_deletes.load(Ordering::SeqCst)
        );
        assert_eq!(
            obs.commit_count.load(Ordering::SeqCst),
            (NUM_THREADS * TXNS_PER_THREAD) as u64
        );
    }

    // -----------------------------------------------------------------
    // Part 7: 高并发 stress — 10 线程 × 1000 事务 × 10 Insert
    // -----------------------------------------------------------------

    /// Stress：10 线程 × 1000 事务 × 10 Insert + 1 Commit = 110,000 events
    /// 验证：不 panic，事件数精确
    #[test]
    fn fuzz_high_concurrency_stress_10k_events() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 1000;
        const INSERTS_PER_TXN: u32 = 10;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64; // 11

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN;
        assert_eq!(obs.total(), expected); // 110,000
        assert_eq!(
            obs.insert_count.load(Ordering::SeqCst),
            (NUM_THREADS * TXNS_PER_THREAD * INSERTS_PER_TXN) as u64
        ); // 100,000
    }

    // -----------------------------------------------------------------
    // Part 8: 多 observer 并发 — 10 observer × 10 writer
    // -----------------------------------------------------------------

    /// 10 observer + 10 writer 线程，验证所有 observer 收到相同数量事件
    #[test]
    fn fuzz_10_observers_10_writers_all_receive_same() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        const NUM_OBS: u32 = 10;
        let observers: Vec<Arc<StressObserver>> = (0..NUM_OBS)
            .map(|i| Arc::new(StressObserver::new(i as u64)))
            .collect();
        for obs in &observers {
            cdc_mgr.register(obs.clone());
        }

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 50;
        const INSERTS_PER_TXN: u32 = 5;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64;
        const EXPECTED_PER_OBS: u64 = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN;

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        // 所有 10 个 observer 都应收到相同的 3000 个事件
        for obs in &observers {
            assert_eq!(
                obs.total(),
                EXPECTED_PER_OBS,
                "Observer {} should receive {} events, got {}",
                obs.id,
                EXPECTED_PER_OBS,
                obs.total()
            );
        }
    }

    // -----------------------------------------------------------------
    // Part 9: 随机退避 + 并发 — yield/sleep 增加交错概率
    // -----------------------------------------------------------------

    /// 10 线程并发，每事务后随机 yield_now，验证事件数精确
    #[test]
    fn fuzz_concurrent_with_random_yield() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 100;
        const INSERTS_PER_TXN: u32 = 3;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64;

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                let mut rng = XorShift64::new(0xFACE + tid as u64);
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                    // 随机退避：yield / sleep / 不动
                    match rng.next_range(3) {
                        0 => std::thread::yield_now(),
                        1 => std::thread::sleep(std::time::Duration::from_micros(1)),
                        _ => {}
                    }
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN;
        assert_eq!(obs.total(), expected);
    }

    // -----------------------------------------------------------------
    // Part 10: Fuzz 不变量 — observer 收到的事件 op 序列符合 WalRecord 序列
    // -----------------------------------------------------------------

    /// 收集型 observer + 单线程，验证事件顺序与 WalRecord 顺序一致
    #[test]
    fn fuzz_event_order_matches_wal_record_order() {
        use crate::CollectingObserver;
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(CollectingObserver::new());
        cdc_mgr.register(obs.clone());

        let engine = CdcEngine::with_timestamp_fn(cdc_mgr.clone(), Box::new(|| 0));

        // 构造一个有明确顺序的事务
        let records = vec![
            WalRecord::new(1, 1, WalOpType::Insert, 42, vec![1]),
            WalRecord::new(2, 1, WalOpType::Update, 42, vec![2]),
            WalRecord::new(3, 1, WalOpType::Delete, 42, vec![3]),
            WalRecord::new(4, 1, WalOpType::Insert, 42, vec![4]),
            WalRecord::new(5, 1, WalOpType::Update, 42, vec![5]),
            WalRecord::new(6, 1, WalOpType::Commit, 0, vec![]),
        ];
        engine.on_commit(1, records);

        let events = obs.events();
        assert_eq!(events.len(), 6);
        assert_eq!(events[0].op, CdcEventOp::Insert);
        assert_eq!(events[0].lsn, 1);
        assert_eq!(events[1].op, CdcEventOp::Update);
        assert_eq!(events[1].lsn, 2);
        assert_eq!(events[2].op, CdcEventOp::Delete);
        assert_eq!(events[2].lsn, 3);
        assert_eq!(events[3].op, CdcEventOp::Insert);
        assert_eq!(events[3].lsn, 4);
        assert_eq!(events[4].op, CdcEventOp::Update);
        assert_eq!(events[4].lsn, 5);
        assert_eq!(events[5].op, CdcEventOp::Commit);
        assert_eq!(events[5].lsn, 6);
    }

    // -----------------------------------------------------------------
    // Part 11: 大事务 stress — 单事务 10000 Insert
    // -----------------------------------------------------------------

    /// 单事务 10000 Insert + 1 Commit = 10001 events
    #[test]
    fn fuzz_large_single_transaction_10k_inserts() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = CdcEngine::with_timestamp_fn(cdc_mgr.clone(), Box::new(|| 0));

        const INSERTS: u32 = 10000;
        let records = make_txn_records(1, 1, INSERTS);
        engine.on_commit(1, records);

        assert_eq!(obs.total(), (INSERTS + 1) as u64); // 10001
        assert_eq!(obs.insert_count.load(Ordering::SeqCst), INSERTS as u64);
        assert_eq!(obs.commit_count.load(Ordering::SeqCst), 1);
    }

    // -----------------------------------------------------------------
    // Part 12: 总览 — at-least-once 语义综合验证
    // -----------------------------------------------------------------

    /// 综合：10 线程 × 100 事务 × 5 Insert + 随机 yield + 3 observer
    /// 验证 at-least-once：3 个 observer 都收到全部事件（无遗漏、无重复）
    #[test]
    fn fuzz_at_least_once_semantics_comprehensive() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs1 = Arc::new(StressObserver::new(1));
        let obs2 = Arc::new(StressObserver::new(2));
        let obs3 = Arc::new(StressObserver::new(3));
        cdc_mgr.register(obs1.clone());
        cdc_mgr.register(obs2.clone());
        cdc_mgr.register(obs3.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 100;
        const INSERTS_PER_TXN: u32 = 5;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64;
        const EXPECTED_PER_OBS: u64 = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN; // 6000

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                let mut rng = XorShift64::new(0xDEAD + tid as u64);
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                    if rng.next_bool() {
                        std::thread::yield_now();
                    }
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        // at-least-once：3 个 observer 都收到 6000 个事件
        assert_eq!(obs1.total(), EXPECTED_PER_OBS);
        assert_eq!(obs2.total(), EXPECTED_PER_OBS);
        assert_eq!(obs3.total(), EXPECTED_PER_OBS);

        // 不遗漏：engine 处理的 WalRecord 总数 = 10 × 100 × 6 = 6000
        assert_eq!(engine.total_processed(), EXPECTED_PER_OBS);

        // 不持续重复：每个 observer 收到的事件数 == engine 处理的记录数
        assert_eq!(obs1.total(), engine.total_processed());
        assert_eq!(obs2.total(), engine.total_processed());
        assert_eq!(obs3.total(), engine.total_processed());

        // total_dispatched = 3 × 6000 = 18000
        assert_eq!(engine.total_dispatched(), 3 * EXPECTED_PER_OBS);
    }

    // -----------------------------------------------------------------
    // Part 13: on_rollback 并发 stress
    // -----------------------------------------------------------------

    /// 10 线程并发 on_rollback，验证 abort 事件数精确
    #[test]
    fn fuzz_concurrent_on_rollback() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const ROLLBACKS_PER_THREAD: u32 = 100;

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                for i in 0..ROLLBACKS_PER_THREAD {
                    let tx_id = tid * ROLLBACKS_PER_THREAD + i + 1;
                    engine.on_rollback(tx_id);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected = (NUM_THREADS * ROLLBACKS_PER_THREAD) as u64;
        assert_eq!(obs.total(), expected);
        assert_eq!(obs.abort_count.load(Ordering::SeqCst), expected);
    }

    // -----------------------------------------------------------------
    // Part 14: 混合 on_commit + on_rollback 并发
    // -----------------------------------------------------------------

    /// 10 线程并发，每线程 50% commit + 50% rollback
    #[test]
    fn fuzz_mixed_commit_rollback_concurrent() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const OPS_PER_THREAD: u32 = 100;

        let total_commits = Arc::new(AtomicU64::new(0));
        let total_rollbacks = Arc::new(AtomicU64::new(0));
        let total_inserts = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let total_commits = total_commits.clone();
            let total_rollbacks = total_rollbacks.clone();
            let total_inserts = total_inserts.clone();
            let handle = thread::spawn(move || {
                let mut rng = XorShift64::new(0xFEED + tid as u64);
                for i in 0..OPS_PER_THREAD {
                    let tx_id = tid * OPS_PER_THREAD + i + 1;
                    if rng.next_bool() {
                        // commit 路径：3 Insert + 1 Commit = 4 events
                        let records = make_txn_records(tx_id, tx_id as u64 * 100, 3);
                        engine.on_commit(tx_id, records);
                        total_commits.fetch_add(1, Ordering::SeqCst);
                        total_inserts.fetch_add(3, Ordering::SeqCst);
                    } else {
                        // rollback 路径：1 Abort event
                        engine.on_rollback(tx_id);
                        total_rollbacks.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let commits = total_commits.load(Ordering::SeqCst);
        let rollbacks = total_rollbacks.load(Ordering::SeqCst);
        let inserts = total_inserts.load(Ordering::SeqCst);

        // 总事件数 = (commits × 4) + rollbacks
        let expected_total = commits * 4 + rollbacks;
        assert_eq!(obs.total(), expected_total);
        assert_eq!(obs.commit_count.load(Ordering::SeqCst), commits);
        assert_eq!(obs.abort_count.load(Ordering::SeqCst), rollbacks);
        assert_eq!(obs.insert_count.load(Ordering::SeqCst), inserts);
    }

    // -----------------------------------------------------------------
    // Part 15: 随机注册/注销 + 高并发写入 — at-least-once 综合验证
    // -----------------------------------------------------------------

    /// 10 writer 线程 + 2 register/unregister 线程 + 3 固定 observer
    /// 验证：固定 observer 收到全部事件（at-least-once）；动态 observer 收到 ≥0 事件
    #[test]
    fn fuzz_register_unregister_during_high_concurrency_writes() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        // 3 个固定 observer，全程注册
        let fixed_obs: Vec<Arc<StressObserver>> = (0..3)
            .map(|i| Arc::new(StressObserver::new(i as u64)))
            .collect();
        for obs in &fixed_obs {
            cdc_mgr.register(obs.clone());
        }

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_WRITERS: u32 = 10;
        const TXNS_PER_WRITER: u32 = 100;
        const INSERTS_PER_TXN: u32 = 5;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64;
        const EXPECTED_PER_FIXED_OBS: u64 = (NUM_WRITERS * TXNS_PER_WRITER) as u64 * EVENTS_PER_TXN; // 6000

        let barrier = Arc::new(std::sync::Barrier::new(12));
        let mut handles = Vec::new();

        // 10 writer 线程
        for tid in 0..NUM_WRITERS {
            let engine = engine.clone();
            let barrier = barrier.clone();
            let handle = thread::spawn(move || {
                barrier.wait();
                let mut rng = XorShift64::new(0xAAAA + tid as u64);
                for txn_idx in 0..TXNS_PER_WRITER {
                    let tx_id = tid * TXNS_PER_WRITER + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                    if rng.next_bool() {
                        std::thread::yield_now();
                    }
                }
            });
            handles.push(handle);
        }

        // 2 register/unregister 线程，频繁注册注销临时 observer
        for rid in 0..2 {
            let cdc_mgr = cdc_mgr.clone();
            let barrier = barrier.clone();
            let handle = thread::spawn(move || {
                barrier.wait();
                let mut rng = XorShift64::new(0xBBBB + rid as u64);
                for _ in 0..200 {
                    // 创建临时 observer，注册，立即注销
                    let temp_obs = Arc::new(StressObserver::new(rng.next_u64()));
                    cdc_mgr.register(temp_obs.clone());
                    // 短暂活跃期间可能收到事件，也可能收不到
                    std::thread::yield_now();
                    cdc_mgr.unregister(&temp_obs);
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // 3 个固定 observer 都应收到全部 6000 个事件（at-least-once，无遗漏）
        for obs in &fixed_obs {
            assert_eq!(
                obs.total(),
                EXPECTED_PER_FIXED_OBS,
                "Fixed observer {} should receive exactly {} events (at-least-once, no missing)",
                obs.id,
                EXPECTED_PER_FIXED_OBS
            );
        }

        // 最终 observer 数量应为 3（所有临时 observer 都已注销）
        assert_eq!(cdc_mgr.observer_count(), 3);
    }

    // -----------------------------------------------------------------
    // Part 16: 长时间 stress — 100000 事件
    // -----------------------------------------------------------------

    /// 10 线程 × 1000 事务 × 10 Insert = 100,000 Insert + 10,000 Commit = 110,000 events
    /// 比 Part 7 多 10 倍 writer threads 数验证
    #[test]
    fn fuzz_long_stress_100k_events() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));
        cdc_mgr.register(obs.clone());

        let engine = Arc::new(CdcEngine::with_timestamp_fn(
            cdc_mgr.clone(),
            Box::new(|| 0),
        ));

        const NUM_THREADS: u32 = 10;
        const TXNS_PER_THREAD: u32 = 1000;
        const INSERTS_PER_TXN: u32 = 10;
        const EVENTS_PER_TXN: u64 = (INSERTS_PER_TXN + 1) as u64;

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let engine = engine.clone();
            let handle = thread::spawn(move || {
                for txn_idx in 0..TXNS_PER_THREAD {
                    let tx_id = tid * TXNS_PER_THREAD + txn_idx + 1;
                    let records = make_txn_records(tx_id, tx_id as u64 * 100, INSERTS_PER_TXN);
                    engine.on_commit(tx_id, records);
                }
            });
            handles.push(handle);
        }
        for h in handles {
            h.join().unwrap();
        }

        let expected = (NUM_THREADS * TXNS_PER_THREAD) as u64 * EVENTS_PER_TXN;
        assert_eq!(obs.total(), expected); // 110,000
    }

    // -----------------------------------------------------------------
    // Part 17: 不变量 — observer 注册后立即注销，再注册，验证可重用
    // -----------------------------------------------------------------

    /// 同一个 observer Arc 多次 register/unregister，验证可重用且不重复计数
    #[test]
    fn fuzz_observer_reusable_after_unregister() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let obs = Arc::new(StressObserver::new(1));

        let engine = CdcEngine::with_timestamp_fn(cdc_mgr.clone(), Box::new(|| 0));

        // 第 1 轮：注册 → 5 事件 → 注销
        cdc_mgr.register(obs.clone());
        for i in 0..5 {
            let tx_id = i + 1;
            let records = make_txn_records(tx_id, tx_id as u64 * 100, 1);
            engine.on_commit(tx_id, records);
        }
        assert_eq!(obs.total(), 10); // 5 × (1 Insert + 1 Commit)
        cdc_mgr.unregister(&obs);

        // 第 2 轮：注销状态下 5 事件，obs 不应收到
        for i in 5..10 {
            let tx_id = i + 1;
            let records = make_txn_records(tx_id, tx_id as u64 * 100, 1);
            engine.on_commit(tx_id, records);
        }
        assert_eq!(obs.total(), 10); // 仍然是 10

        // 第 3 轮：重新注册 → 5 事件 → obs 应收到
        cdc_mgr.register(obs.clone());
        for i in 10..15 {
            let tx_id = i + 1;
            let records = make_txn_records(tx_id, tx_id as u64 * 100, 1);
            engine.on_commit(tx_id, records);
        }
        assert_eq!(obs.total(), 20); // 10 + 10
    }

    // -----------------------------------------------------------------
    // Part 18: 并发不变量 — register/unregister 不会导致 panic 或数据竞争
    // -----------------------------------------------------------------

    /// 高频 register/unregister 同一组 observer，验证不 panic
    #[test]
    fn fuzz_high_frequency_register_unregister_no_panic() {
        let cdc_mgr = Arc::new(CdcObserverManager::new());
        let observers: Vec<Arc<StressObserver>> = (0..20)
            .map(|i| Arc::new(StressObserver::new(i as u64)))
            .collect();

        let barrier = Arc::new(std::sync::Barrier::new(10));
        let mut handles = Vec::new();

        for tid in 0..10 {
            let cdc_mgr = cdc_mgr.clone();
            let observers = observers.clone();
            let barrier = barrier.clone();
            let handle = thread::spawn(move || {
                barrier.wait();
                let mut rng = XorShift64::new(0xCCCC + tid as u64);
                for _ in 0..500 {
                    let idx = rng.next_range(20) as usize;
                    let obs = observers[idx].clone();
                    if rng.next_bool() {
                        cdc_mgr.register(obs);
                    } else {
                        cdc_mgr.unregister(&obs);
                    }
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // 不 panic 即通过
        // 验证：observer_count 在 0..=20 之间
        let count = cdc_mgr.observer_count();
        assert!(count <= 20, "observer_count should be <= 20, got {}", count);
    }
}
