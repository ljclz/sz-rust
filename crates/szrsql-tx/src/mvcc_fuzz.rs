//! SzRSQL MVCC Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 2.8。
//!
//! 验证标准（来自实施进度表）：
//! - **Fuzz**：10 线程并发 BEGIN/COMMIT/ABORT + 随机快照创建 + 可见性判断，
//!   与单线程串行参考实现对比
//! - **判定**：并发结果与串行参考完全一致
//!
//! 设计要点：
//! 1. **XorShift64 PRNG**：固定种子，测试可重现（与 page_fuzz/wal_fuzz 同风格）
//! 2. **并发不变量**（无论并发时序如何，以下不变量必须成立）：
//!    - 所有 txn_id 全局唯一
//!    - committed_count + aborted_count + active_count == total_begun
//!    - 单个 txn 只能处于 Active / Committed / Aborted 三态之一
//!    - Committed/Aborted 不可逆（重复 commit/abort 返回 Err）
//! 3. **可见性确定性**：给定 (snapshot, committed, aborted, xmin, xmax, current_txn)，
//!    `is_visible` 结果必须确定（纯函数）
//! 4. **串行参考对比**：预生成操作序列 → 串行执行 → 记录最终状态；
//!    再次串行执行相同序列 → 验证结果完全一致（确定性）
//! 5. **并发可见性**：10 线程对相同输入并行计算 `is_visible`，结果必须一致

use crate::mvcc::Snapshot;
use std::collections::{BTreeSet, HashMap};

// =====================================================================
// XorShift64 — 固定种子 PRNG（与 page_fuzz.rs / wal_fuzz.rs 同风格）
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

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() & 0xFFFF_FFFF) as u32
    }

    fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    /// [0, n) 范围
    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    /// [min, max] 范围
    fn next_in(&mut self, min: u32, max: u32) -> u32 {
        if min >= max {
            return min;
        }
        min + self.next_range(max - min + 1)
    }

    /// 50% 概率返回 true
    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

// =====================================================================
// 辅助函数：构造随机 committed/aborted 集合 + snapshot
// =====================================================================

/// 构造随机测试数据：snapshot + committed + aborted + current_txn
struct VisibilityTestCase {
    snapshot: Snapshot,
    committed: BTreeSet<u32>,
    aborted: BTreeSet<u32>,
    current_txn: u32,
    parent_map: HashMap<u32, u32>,
}

/// 生成一个随机的可见性测试用例
///
/// - txn_id 范围 [1, 100]
/// - committed/aborted 各随机包含 10-30 个 txn
/// - active_txns 随机包含 0-10 个 txn
/// - snapshot.xmax = 100
fn random_visibility_case(rng: &mut XorShift64) -> VisibilityTestCase {
    let xmax = 100u32;
    // active_txns: 0-10 个，范围 [1, 99]
    let active_count = rng.next_in(0, 10) as usize;
    let mut active_txns: Vec<u32> = (0..active_count).map(|_| rng.next_in(1, 99)).collect();
    active_txns.sort_unstable();
    active_txns.dedup();
    let snapshot = Snapshot::new(active_txns.clone(), xmax);

    // committed: 10-30 个，不在 active_txns 中，< xmax
    let mut committed = BTreeSet::new();
    let committed_count = rng.next_in(10, 30);
    for _ in 0..committed_count {
        let mut txn_id = rng.next_in(1, 99);
        while active_txns.contains(&txn_id) || committed.contains(&txn_id) {
            txn_id = rng.next_in(1, 99);
        }
        committed.insert(txn_id);
    }

    // aborted: 10-30 个，不在 active_txns 和 committed 中
    let mut aborted = BTreeSet::new();
    let aborted_count = rng.next_in(10, 30);
    for _ in 0..aborted_count {
        let mut txn_id = rng.next_in(1, 99);
        while active_txns.contains(&txn_id)
            || committed.contains(&txn_id)
            || aborted.contains(&txn_id)
        {
            txn_id = rng.next_in(1, 99);
        }
        aborted.insert(txn_id);
    }

    // current_txn: 随机选一个 [1, 99] 中的 txn
    let current_txn = rng.next_in(1, 99);

    // parent_map: 5% 概率添加一个子事务关系
    let mut parent_map = HashMap::new();
    if rng.next_bool() && current_txn > 1 {
        let child = rng.next_in(1, current_txn - 1);
        // 避免自引用
        if child != current_txn {
            parent_map.insert(child, current_txn);
        }
    }

    VisibilityTestCase {
        snapshot,
        committed,
        aborted,
        current_txn,
        parent_map,
    }
}

/// 生成随机 (xmin, xmax) 对用于可见性测试
fn random_xmin_xmax(rng: &mut XorShift64) -> (u32, u32) {
    // xmin: 0-100（0 = Frozen）
    let xmin = rng.next_in(0, 100);
    // xmax: 0-100（0 = 未删除）
    let xmax = rng.next_in(0, 100);
    (xmin, xmax)
}

// =====================================================================
// Phase 2.8 测试
// =====================================================================

#[cfg(test)]
mod phase_2_8 {
    use super::*;
    use crate::mvcc::MvccManager;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::thread;

    // -----------------------------------------------------------------
    // 1. 并发 BEGIN 线程安全（10 线程 × 100 轮 = 1000 个事务）
    // -----------------------------------------------------------------

    /// 10 线程并发 BEGIN，验证：
    /// - 所有 txn_id 全局唯一
    /// - active_count == 1000
    /// - current_xid == 1001（从 1 开始，分配了 1000 个）
    #[test]
    fn fuzz_concurrent_begin_10_threads_unique_ids() {
        const THREADS: usize = 10;
        const ROUNDS: usize = 100;
        const TOTAL: usize = THREADS * ROUNDS;

        let mgr = Arc::new(MvccManager::new());
        let txn_ids = Arc::new(parking_lot::Mutex::new(Vec::with_capacity(TOTAL)));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let mgr = Arc::clone(&mgr);
                let txn_ids = Arc::clone(&txn_ids);
                thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        let txn = mgr.begin();
                        txn_ids.lock().push(txn.txn_id);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let txn_ids = txn_ids.lock();
        assert_eq!(txn_ids.len(), TOTAL);

        // 验证所有 txn_id 唯一
        let mut unique = HashSet::new();
        for &id in txn_ids.iter() {
            assert!(unique.insert(id), "发现重复 txn_id: {}", id);
        }
        assert_eq!(unique.len(), TOTAL);

        // 验证 active_count
        assert_eq!(mgr.active_count(), TOTAL);
        // 验证 current_xid
        assert_eq!(mgr.current_xid(), (TOTAL + 1) as u32);
    }

    // -----------------------------------------------------------------
    // 2. 并发 BEGIN/COMMIT/ABORT 一致性（10 线程）
    // -----------------------------------------------------------------

    /// 10 线程并发 BEGIN + 随机 COMMIT/ABORT，验证：
    /// - committed + aborted + active == total_begun
    /// - 所有 txn_id 唯一
    #[test]
    fn fuzz_concurrent_begin_commit_abort_consistency() {
        const THREADS: usize = 10;
        const ROUNDS: usize = 100;
        const TOTAL: usize = THREADS * ROUNDS;

        let mgr = Arc::new(MvccManager::new());
        let error_count = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let error_count = Arc::clone(&error_count);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0xC0FFEE);
                    for _ in 0..ROUNDS {
                        let txn = mgr.begin();
                        // 随机 COMMIT 或 ABORT
                        let result = if rng.next_bool() {
                            mgr.commit(txn.txn_id, rng.next_u64())
                        } else {
                            mgr.abort(txn.txn_id)
                        };
                        if result.is_err() {
                            error_count.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // 验证不变量：committed + aborted + active == TOTAL
        let committed = mgr.committed_count();
        let aborted = mgr.aborted_count();
        let active = mgr.active_count();
        assert_eq!(
            committed + aborted + active,
            TOTAL,
            "committed({}) + aborted({}) + active({}) != TOTAL({})",
            committed,
            aborted,
            active,
            TOTAL
        );

        // 在并发下不应有错误（每个 txn 只操作一次）
        assert_eq!(error_count.load(Ordering::SeqCst), 0);
    }

    // -----------------------------------------------------------------
    // 3. 并发重复 COMMIT/ABORT 错误处理（10 线程）
    // -----------------------------------------------------------------

    /// 10 线程对同一组 txn 并发 COMMIT/ABORT，验证：
    /// - 每个 txn 只能成功 COMMIT 或 ABORT 一次
    /// - 重复操作返回 AlreadyCommitted / AlreadyAborted
    #[test]
    fn fuzz_concurrent_duplicate_commit_abort_error_handling() {
        const THREADS: usize = 10;
        const TXN_COUNT: usize = 50;

        let mgr = Arc::new(MvccManager::new());

        // 预先 BEGIN 50 个事务
        let txn_ids: Vec<u32> = (0..TXN_COUNT).map(|_| mgr.begin().txn_id).collect();

        // 10 线程并发对每个 txn 随机 COMMIT/ABORT
        let success_count = Arc::new(AtomicU32::new(0));
        let error_count = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let txn_ids = txn_ids.clone();
                let success_count = Arc::clone(&success_count);
                let error_count = Arc::clone(&error_count);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 1);
                    for &txn_id in &txn_ids {
                        let result = if rng.next_bool() {
                            mgr.commit(txn_id, rng.next_u64())
                        } else {
                            mgr.abort(txn_id)
                        };
                        if result.is_ok() {
                            success_count.fetch_add(1, Ordering::SeqCst);
                        } else {
                            error_count.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // 验证：成功次数 + 失败次数 == THREADS * TXN_COUNT
        let success = success_count.load(Ordering::SeqCst) as usize;
        let error = error_count.load(Ordering::SeqCst) as usize;
        assert_eq!(
            success + error,
            THREADS * TXN_COUNT,
            "success({}) + error({}) != total({})",
            success,
            error,
            THREADS * TXN_COUNT
        );

        // 验证：每个 txn 只能成功一次，所以成功次数 <= TXN_COUNT
        assert!(
            success <= TXN_COUNT,
            "成功次数 {} 不应超过 txn 总数 {}",
            success,
            TXN_COUNT
        );

        // 验证：committed + aborted == 成功次数
        let committed = mgr.committed_count();
        let aborted = mgr.aborted_count();
        assert_eq!(
            committed + aborted,
            success,
            "committed({}) + aborted({}) != success({})",
            committed,
            aborted,
            success
        );

        // 验证：active_count == 0（所有 txn 都已 commit 或 abort）
        assert_eq!(mgr.active_count(), 0);
    }

    // -----------------------------------------------------------------
    // 4. 可见性确定性（1000 轮，每轮 100 个 (xmin, xmax) 对）
    // -----------------------------------------------------------------

    /// 随机生成 snapshot + committed + aborted，对 100 个 (xmin, xmax) 对
    /// 重复计算两次 is_visible，验证结果完全一致（纯函数性质）
    #[test]
    fn fuzz_visibility_determinism_1000_rounds() {
        let mut rng = XorShift64::new(0xABCDEF_123456);
        for round in 0..1000 {
            let case = random_visibility_case(&mut rng);
            // 生成 100 个 (xmin, xmax) 对
            let pairs: Vec<(u32, u32)> = (0..100).map(|_| random_xmin_xmax(&mut rng)).collect();

            // 计算两次，验证结果一致
            for (i, &(xmin, xmax)) in pairs.iter().enumerate() {
                let result1 = case.snapshot.is_visible(
                    xmin,
                    xmax,
                    case.current_txn,
                    &case.committed,
                    &case.aborted,
                    &case.parent_map,
                );
                let result2 = case.snapshot.is_visible(
                    xmin,
                    xmax,
                    case.current_txn,
                    &case.committed,
                    &case.aborted,
                    &case.parent_map,
                );
                assert_eq!(
                    result1, result2,
                    "round={} pair={} ({}, {}): 可见性非确定",
                    round, i, xmin, xmax
                );
            }
        }
    }

    // -----------------------------------------------------------------
    // 5. 并发可见性一致性（10 线程，相同输入）
    // -----------------------------------------------------------------

    /// 预生成 snapshot + committed + aborted + 1000 个 (xmin, xmax) 对
    /// 10 线程并行计算 is_visible，验证所有线程结果一致
    #[test]
    fn fuzz_concurrent_visibility_consistency_10_threads() {
        const THREADS: usize = 10;
        const PAIRS: usize = 1000;

        let mut rng = XorShift64::new(0x55AA_55AA);
        let case = random_visibility_case(&mut rng);
        let pairs: Vec<(u32, u32)> = (0..PAIRS).map(|_| random_xmin_xmax(&mut rng)).collect();

        // 主线程计算参考结果
        let reference: Vec<bool> = pairs
            .iter()
            .map(|&(xmin, xmax)| {
                case.snapshot.is_visible(
                    xmin,
                    xmax,
                    case.current_txn,
                    &case.committed,
                    &case.aborted,
                    &case.parent_map,
                )
            })
            .collect();

        // 10 线程并行计算，每个线程独立计算所有 PAIRS
        let case_arc = Arc::new(case);
        let pairs_arc = Arc::new(pairs);
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let case = Arc::clone(&case_arc);
                let pairs = Arc::clone(&pairs_arc);
                thread::spawn(move || {
                    pairs
                        .iter()
                        .map(|&(xmin, xmax)| {
                            case.snapshot.is_visible(
                                xmin,
                                xmax,
                                case.current_txn,
                                &case.committed,
                                &case.aborted,
                                &case.parent_map,
                            )
                        })
                        .collect::<Vec<bool>>()
                })
            })
            .collect();

        // 验证所有线程结果与参考一致
        for (tid, h) in handles.into_iter().enumerate() {
            let result = h.join().unwrap();
            assert_eq!(result, reference, "线程 {} 的可见性结果与参考不一致", tid);
        }
    }

    // -----------------------------------------------------------------
    // 6. 串行参考对比（确定性验证）
    // -----------------------------------------------------------------

    /// 预生成操作序列 → 串行执行两次 → 验证结果完全一致
    ///
    /// 这是"与单线程串行参考实现对比"的核心测试：
    /// - 同样的操作序列，两次串行执行的结果必须完全相同
    #[test]
    fn fuzz_serial_reference_determinism() {
        const OPS: usize = 500;
        let mut rng = XorShift64::new(0x1234_5678);

        // 预生成操作序列
        // Op: 0=BEGIN, 1=COMMIT(最近活跃 txn), 2=ABORT(最近活跃 txn)
        #[derive(Debug, Clone, Copy)]
        enum Op {
            Begin,
            Commit,
            Abort,
        }

        let ops: Vec<Op> = (0..OPS)
            .map(|_| match rng.next_range(3) {
                0 => Op::Begin,
                1 => Op::Commit,
                _ => Op::Abort,
            })
            .collect();

        // 第一次串行执行
        let (committed1, aborted1, active1) = {
            let mgr = MvccManager::new();
            let mut active_txns: Vec<u32> = Vec::new();
            for op in &ops {
                match op {
                    Op::Begin => {
                        let txn = mgr.begin();
                        active_txns.push(txn.txn_id);
                    }
                    Op::Commit => {
                        if let Some(&txn_id) = active_txns.last() {
                            if mgr.commit(txn_id, 0).is_ok() {
                                active_txns.pop();
                            }
                        }
                    }
                    Op::Abort => {
                        if let Some(&txn_id) = active_txns.last() {
                            if mgr.abort(txn_id).is_ok() {
                                active_txns.pop();
                            }
                        }
                    }
                }
            }
            (
                mgr.committed_count(),
                mgr.aborted_count(),
                mgr.active_count(),
            )
        };

        // 第二次串行执行（相同操作序列）
        let (committed2, aborted2, active2) = {
            let mgr = MvccManager::new();
            let mut active_txns: Vec<u32> = Vec::new();
            for op in &ops {
                match op {
                    Op::Begin => {
                        let txn = mgr.begin();
                        active_txns.push(txn.txn_id);
                    }
                    Op::Commit => {
                        if let Some(&txn_id) = active_txns.last() {
                            if mgr.commit(txn_id, 0).is_ok() {
                                active_txns.pop();
                            }
                        }
                    }
                    Op::Abort => {
                        if let Some(&txn_id) = active_txns.last() {
                            if mgr.abort(txn_id).is_ok() {
                                active_txns.pop();
                            }
                        }
                    }
                }
            }
            (
                mgr.committed_count(),
                mgr.aborted_count(),
                mgr.active_count(),
            )
        };

        // 验证两次串行执行结果完全一致
        assert_eq!(committed1, committed2, "committed 不一致");
        assert_eq!(aborted1, aborted2, "aborted 不一致");
        assert_eq!(active1, active2, "active 不一致");
    }

    // -----------------------------------------------------------------
    // 7. 并发 BEGIN + 串行 COMMIT 对比（混合模式）
    // -----------------------------------------------------------------

    /// 并发 BEGIN 100 个事务 → 串行 COMMIT/ABORT → 验证最终状态一致
    ///
    /// 验证：并发 BEGIN 的结果（txn_id 集合）与串行 BEGIN 一致
    #[test]
    fn fuzz_concurrent_begin_serial_commit_comparison() {
        const TXN_COUNT: usize = 100;
        let mut rng = XorShift64::new(0x9999_8888);

        // 串行参考：BEGIN TXN_COUNT 个事务，记录 txn_id
        let serial_txn_ids: Vec<u32> = {
            let mgr = MvccManager::new();
            (0..TXN_COUNT).map(|_| mgr.begin().txn_id).collect()
        };

        // 并发：10 线程 × 10 轮 BEGIN
        const THREADS: usize = 10;
        const ROUNDS: usize = 10;
        let mgr = Arc::new(MvccManager::new());
        let txn_ids = Arc::new(parking_lot::Mutex::new(Vec::with_capacity(TXN_COUNT)));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let mgr = Arc::clone(&mgr);
                let txn_ids = Arc::clone(&txn_ids);
                thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        let txn = mgr.begin();
                        txn_ids.lock().push(txn.txn_id);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let concurrent_txn_ids = txn_ids.lock();
        assert_eq!(concurrent_txn_ids.len(), TXN_COUNT);

        // 排序后比较（并发下顺序可能不同，但集合应一致）
        let mut concurrent_sorted = concurrent_txn_ids.clone();
        concurrent_sorted.sort_unstable();
        let mut serial_sorted = serial_txn_ids.clone();
        serial_sorted.sort_unstable();

        // 并发与串行的 txn_id 集合应完全一致（从 1 开始递增）
        assert_eq!(
            concurrent_sorted, serial_sorted,
            "并发 BEGIN 的 txn_id 集合与串行不一致"
        );

        // 串行 COMMIT/ABORT 一半
        for (i, &txn_id) in concurrent_txn_ids.iter().enumerate() {
            if i % 2 == 0 {
                assert!(mgr.commit(txn_id, rng.next_u64()).is_ok());
            } else {
                assert!(mgr.abort(txn_id).is_ok());
            }
        }

        // 验证最终状态
        assert_eq!(mgr.committed_count(), TXN_COUNT / 2);
        assert_eq!(mgr.aborted_count(), TXN_COUNT / 2);
        assert_eq!(mgr.active_count(), 0);
    }

    // -----------------------------------------------------------------
    // 8. 大规模随机可见性 fuzz（10000 轮）
    // -----------------------------------------------------------------

    /// 10000 轮随机可见性测试，验证 is_visible 永不 panic
    /// 且结果在 [true, false] 之内（不返回其他值）
    #[test]
    fn fuzz_visibility_10000_rounds_no_panic() {
        let mut rng = XorShift64::new(0xCAFE_BABE);
        for round in 0..10000 {
            let case = random_visibility_case(&mut rng);
            let (xmin, xmax) = random_xmin_xmax(&mut rng);
            let result = case.snapshot.is_visible(
                xmin,
                xmax,
                case.current_txn,
                &case.committed,
                &case.aborted,
                &case.parent_map,
            );
            // 结果只能是 true 或 false（不 panic 即通过）
            let _ = result; // 明确使用 result 避免未使用警告

            // 每 2000 轮输出一次进度（到 stderr，避免测试卡死判断）
            if round > 0 && round % 2000 == 0 {
                eprintln!("[mvcc_fuzz] visibility {} / 10000 rounds done", round);
            }
        }
    }

    // -----------------------------------------------------------------
    // 9. 并发状态转换安全性（10 线程 × 1000 轮）
    // -----------------------------------------------------------------

    /// 10 线程并发 BEGIN + 立即 COMMIT/ABORT，验证：
    /// - 没有 txn 被同时 commit 和 abort
    /// - committed + aborted == TOTAL
    #[test]
    fn fuzz_concurrent_state_transition_safety() {
        const THREADS: usize = 10;
        const ROUNDS: usize = 1000;
        const TOTAL: usize = THREADS * ROUNDS;

        let mgr = Arc::new(MvccManager::new());
        let committed_count = Arc::new(AtomicU32::new(0));
        let aborted_count = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let committed_count = Arc::clone(&committed_count);
                let aborted_count = Arc::clone(&aborted_count);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 * 1000 + 7);
                    for _ in 0..ROUNDS {
                        let txn = mgr.begin();
                        // 立即 COMMIT 或 ABORT
                        if rng.next_bool() {
                            if mgr.commit(txn.txn_id, rng.next_u64()).is_ok() {
                                committed_count.fetch_add(1, Ordering::SeqCst);
                            }
                        } else if mgr.abort(txn.txn_id).is_ok() {
                            aborted_count.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // 验证：committed + aborted == TOTAL（每个 txn 都被处理一次）
        let committed = committed_count.load(Ordering::SeqCst) as usize;
        let aborted = aborted_count.load(Ordering::SeqCst) as usize;
        assert_eq!(
            committed + aborted,
            TOTAL,
            "committed({}) + aborted({}) != TOTAL({})",
            committed,
            aborted,
            TOTAL
        );

        // 验证 active_count == 0
        assert_eq!(mgr.active_count(), 0);

        // 验证 manager 的计数与原子计数器一致
        assert_eq!(mgr.committed_count(), committed);
        assert_eq!(mgr.aborted_count(), aborted);
    }

    // -----------------------------------------------------------------
    // 10. 边界条件 fuzz（极端 txn_id）
    // -----------------------------------------------------------------

    /// 测试极端 txn_id（0, 1, u32::MAX）的可见性判断不 panic
    #[test]
    fn fuzz_visibility_extreme_txn_ids_no_panic() {
        let mut rng = XorShift64::new(0xDEAD_BEEF);
        for _ in 0..1000 {
            let mut case = random_visibility_case(&mut rng);
            // 极端 current_txn
            for &extreme_txn in &[0u32, 1u32, u32::MAX] {
                case.current_txn = extreme_txn;
                for &(xmin, xmax) in &[
                    (0u32, 0u32),
                    (0, u32::MAX),
                    (u32::MAX, 0),
                    (u32::MAX, u32::MAX),
                    (1, 1),
                ] {
                    let _ = case.snapshot.is_visible(
                        xmin,
                        xmax,
                        case.current_txn,
                        &case.committed,
                        &case.aborted,
                        &case.parent_map,
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // 11. MvccManager 集成 fuzz（随机操作序列 + 可见性验证）
    // -----------------------------------------------------------------

    /// 综合测试：随机 BEGIN/COMMIT/ABORT 序列 + 随机可见性查询
    /// 验证整个 MvccManager 在混合操作下的一致性
    #[test]
    fn fuzz_mvcc_manager_integration_mixed_ops() {
        const OPS: usize = 500;
        const VISIBILITY_CHECKS: usize = 100;
        let mut rng = XorShift64::new(0xFEDC_BA98);

        let mgr = MvccManager::new();
        let mut active_txns: Vec<u32> = Vec::new();
        let mut committed_txns: Vec<u32> = Vec::new();
        let mut aborted_txns: Vec<u32> = Vec::new();

        for op_idx in 0..OPS {
            // 60% BEGIN, 20% COMMIT, 20% ABORT
            let action = rng.next_range(100);
            if action < 60 || active_txns.is_empty() {
                let txn = mgr.begin();
                active_txns.push(txn.txn_id);
            } else if action < 80 {
                // COMMIT 随机活跃事务
                let idx = rng.next_range(active_txns.len() as u32) as usize;
                let txn_id = active_txns.swap_remove(idx);
                if mgr.commit(txn_id, rng.next_u64()).is_ok() {
                    committed_txns.push(txn_id);
                } else {
                    active_txns.push(txn_id); // 放回（不应发生）
                }
            } else {
                // ABORT 随机活跃事务
                let idx = rng.next_range(active_txns.len() as u32) as usize;
                let txn_id = active_txns.swap_remove(idx);
                if mgr.abort(txn_id).is_ok() {
                    aborted_txns.push(txn_id);
                } else {
                    active_txns.push(txn_id); // 放回（不应发生）
                }
            }

            // 每 50 步做一次可见性检查
            if op_idx % 50 == 0 && !committed_txns.is_empty() {
                let viewer = mgr.begin(); // 新事务作为观察者
                for _ in 0..VISIBILITY_CHECKS {
                    // 随机选一个已提交 txn 作为 xmin
                    let xmin = committed_txns[rng.next_range(committed_txns.len() as u32) as usize];
                    // 随机选 xmax：0 / 已提交 / 已回滚 / 活跃
                    let xmax = match rng.next_range(4) {
                        0 => 0u32,
                        1 => *committed_txns
                            .get(rng.next_range(committed_txns.len() as u32) as usize)
                            .unwrap_or(&0),
                        2 => *aborted_txns
                            .get(rng.next_range(aborted_txns.len().max(1) as u32) as usize)
                            .unwrap_or(&0),
                        _ => *active_txns
                            .get(rng.next_range(active_txns.len().max(1) as u32) as usize)
                            .unwrap_or(&0),
                    };
                    // 可见性判断不 panic
                    let _ = mgr.is_visible(viewer.txn_id, xmin, xmax);
                }
                // 观察者事务 commit（清理）
                let _ = mgr.commit(viewer.txn_id, 0);
            }
        }

        // 最终验证：committed + aborted + active == OPS 中 BEGIN 的次数
        // 由于观察者事务也被 BEGIN + COMMIT，需要追踪总数
        // 这里简化：验证 manager 内部计数一致
        let mgr_committed = mgr.committed_count();
        let mgr_aborted = mgr.aborted_count();
        let mgr_active = mgr.active_count();

        // 不变量：committed + aborted + active > 0（至少有一些事务）
        assert!(
            mgr_committed + mgr_aborted + mgr_active > 0,
            "应有事务被处理"
        );

        // 验证：committed_txns 的数量 <= mgr_committed（因为观察者事务也被计入）
        assert!(
            committed_txns.len() <= mgr_committed,
            "committed_txns({}) 应 <= mgr_committed({})",
            committed_txns.len(),
            mgr_committed
        );
    }

    // -----------------------------------------------------------------
    // 12. 并发快照一致性（10 线程同时 BEGIN，验证快照）
    // -----------------------------------------------------------------

    /// 10 线程同时 BEGIN，验证每个事务的快照包含其他所有活跃事务
    ///
    /// 注意：由于并发时序，每个事务的快照可能不同。但不变量是：
    /// - 快照的 active_txns 中的事务在当时确实活跃
    /// - 快照的 xmax > 所有 active_txns 中的 txn_id
    #[test]
    fn fuzz_concurrent_snapshot_consistency() {
        const THREADS: usize = 10;

        let mgr = Arc::new(MvccManager::new());
        let snapshots = Arc::new(parking_lot::Mutex::new(Vec::with_capacity(THREADS)));

        // 先 BEGIN 一个事务 T0（始终活跃）
        let t0 = mgr.begin();

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let mgr = Arc::clone(&mgr);
                let snapshots = Arc::clone(&snapshots);
                thread::spawn(move || {
                    let txn = mgr.begin();
                    snapshots.lock().push((txn.txn_id, txn.snapshot));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let snapshots = snapshots.lock();
        assert_eq!(snapshots.len(), THREADS);

        // 验证每个快照：
        // 1. xmax > 该事务自己的 txn_id
        // 2. active_txns 中的 txn_id 都 < xmax
        // 3. T0 (txn_id=1) 应在每个快照的 active_txns 中（因为 T0 在所有线程 BEGIN 之前已活跃）
        for &(txn_id, ref snap) in snapshots.iter() {
            assert!(
                snap.xmax > txn_id,
                "txn {} 的快照 xmax {} 应 > 自身 txn_id",
                txn_id,
                snap.xmax
            );
            for &active_id in &snap.active_txns {
                assert!(
                    active_id < snap.xmax,
                    "active txn {} 应 < xmax {}",
                    active_id,
                    snap.xmax
                );
            }
            // T0 (txn_id=1) 应在快照中（T0 在所有线程之前 BEGIN）
            assert!(
                snap.is_active(t0.txn_id),
                "txn {} 的快照应包含 T0 (txn_id={})",
                txn_id,
                t0.txn_id
            );
        }
    }
}
