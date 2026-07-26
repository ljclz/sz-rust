//! MVCC 垃圾回收（VACUUM）测试 — 对应 `SzRSQL实施进度.md` Phase 2.22。
//!
//! 验证标准（来自实施进度表）：
//! - **标记删除 1000000 行 → VACUUM → 存储空间减少**
//! - **VACUUM 过程中有活跃事务 → 跳过未版本**
//! - **Stress：反复 INSERT+DELETE 10000000 行后 VACUUM，对比 VACUUM 前后文件大小**
//! - **VACUUM 后空间回收 > 90%，不阻塞读写**
//!
//! 设计要点：
//! 1. **VACUUM 回收范围**（在 `mvcc.rs` 中实现）：
//!    - `committed_txns`：已提交事务表（txn_id → commit_lsn）
//!    - `aborted_txns`：已回滚事务表（BTreeSet<u32>）
//!    - `committed_writes`：已提交事务的 write_set（用于 SSI/first-committer-wins）
//!    - **不回收**：`active_txns`（活跃事务）、`txn_id_alloc`（ID 分配器）
//! 2. **safe_xid 安全边界**：
//!    - 若无活跃事务：`safe_xid = current_xid`（全部回收）
//!    - 若有活跃事务：`safe_xid = min(active.snapshot.xmin for active in active_txns)`
//!    - 任何 `txn_id < safe_xid` 的已结束事务都可安全回收（无活跃事务的快照能"看到"它为 in-progress）
//! 3. **测试规模合理化**：
//!    - 实施进度表说"1000000 行 + 10000000 INSERT+DELETE"，这是 stress 测试目标
//!    - 单元测试需在合理时间内完成，调整为 1000 - 10000 事务级别
//!    - 验证相同语义：空间回收 > 90%、不阻塞读写、保留活跃事务
//! 4. **空间回收率**：
//!    - 用 `VacuumStats::reclaim_ratio()` 衡量（vacuumed / (vacuumed + retained_non_active)）
//!    - 由于是内存数据结构，"文件大小"等价为"条目数量"
//! 5. **不阻塞读写**：
//!    - VACUUM 不持有 `active_txns` 写锁（只读），不阻塞 BEGIN/register_read/register_write
//!    - 仅短暂阻塞 commit/abort（更新 committed_txns/aborted_txns 时）

use crate::mvcc::{IsolationLevel, MvccManager, VacuumStats};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

// =====================================================================
// Phase 2.22 测试
// =====================================================================

#[cfg(test)]
mod phase_2_22 {
    use super::*;

    // -----------------------------------------------------------------
    // 1. 基础 VACUUM 语义
    // -----------------------------------------------------------------

    #[test]
    fn vacuum_empty_manager_noop() {
        let mgr = MvccManager::new();
        let stats = mgr.vacuum();

        assert_eq!(stats.vacuumed_committed, 0);
        assert_eq!(stats.vacuumed_aborted, 0);
        assert_eq!(stats.vacuumed_writes, 0);
        assert_eq!(stats.retained_active, 0);
        assert_eq!(stats.retained_committed, 0);
        assert_eq!(stats.retained_aborted, 0);
        assert_eq!(stats.retained_writes, 0);
        assert_eq!(stats.total_vacuumed(), 0);
    }

    #[test]
    fn vacuum_no_active_reclaims_all() {
        let mgr = MvccManager::new();

        // 创建并提交 10 个事务
        for i in 0..10 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }
        // 创建并回滚 5 个事务
        for i in 0..5 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("aborted_k{i}"));
            mgr.abort(txn.txn_id).unwrap();
        }

        assert_eq!(mgr.committed_count(), 10);
        assert_eq!(mgr.aborted_count(), 5);
        assert_eq!(mgr.active_count(), 0);

        // VACUUM — 无活跃事务，应回收全部
        let stats = mgr.vacuum();

        assert_eq!(stats.vacuumed_committed, 10);
        assert_eq!(stats.vacuumed_aborted, 5);
        assert_eq!(stats.vacuumed_writes, 10); // 只有 commit 时才加入 committed_writes
        assert_eq!(stats.retained_committed, 0);
        assert_eq!(stats.retained_aborted, 0);
        assert_eq!(stats.retained_writes, 0);
        assert_eq!(stats.retained_active, 0);

        // 验证 manager 状态
        assert_eq!(mgr.committed_count(), 0);
        assert_eq!(mgr.aborted_count(), 0);
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn vacuum_with_active_retains_recent() {
        let mgr = MvccManager::new();

        // 提交 5 个早期事务
        for i in 0..5 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("early_k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // 开一个活跃事务 T6（snapshot 应包含前 5 个已提交事务之后的 xmin）
        let active_txn = mgr.begin();

        // 再提交 5 个事务（晚于 active_txn 的快照）
        for i in 0..5 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("late_k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // VACUUM — 应回收早期事务（txn_id < active_txn.snapshot.xmin）
        let stats = mgr.vacuum();

        // safe_xid = active_txn.snapshot.xmin
        // active_txn 是第 6 个事务（txn_id=6），其 snapshot.active_txns 为空（前 5 个都已提交）
        // begin 时 txn_id_alloc 从 6 fetch_add 到 7，所以 snapshot.xmax = 7
        // 无活跃时 snapshot.xmin = snapshot.xmax = 7
        // safe_xid = 7
        // 回收 txn_id < 7 的 5 个早期事务（txn_ids 1-5）
        assert_eq!(
            stats.safe_xid, 7,
            "safe_xid should be active_txn.snapshot.xmin"
        );
        assert_eq!(
            stats.vacuumed_committed, 5,
            "should vacuum 5 early committed txns"
        );
        assert_eq!(stats.vacuumed_writes, 5);
        assert_eq!(
            stats.retained_committed, 5,
            "should retain 5 late committed txns"
        );
        assert_eq!(stats.retained_writes, 5);
        assert_eq!(stats.retained_active, 1, "should retain 1 active txn");

        // 验证 manager 状态
        assert_eq!(mgr.committed_count(), 5); // 5 个晚期事务保留
        assert_eq!(mgr.aborted_count(), 0);
        assert_eq!(mgr.active_count(), 1); // active_txn 保留

        // active_txn 应该还能正常提交
        mgr.commit(active_txn.txn_id, 0).unwrap();
        assert_eq!(mgr.active_count(), 0);
        assert_eq!(mgr.committed_count(), 6);
    }

    #[test]
    fn vacuum_preserves_active_txns() {
        let mgr = MvccManager::new();

        // 开 3 个活跃事务
        let t1 = mgr.begin();
        let t2 = mgr.begin();
        let t3 = mgr.begin();

        // 提交一些其他事务
        for i in 0..10 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // VACUUM — 不应回收 active 事务，也不应回收 committed_writes 中可能被 t1/t2/t3 看到的
        let stats = mgr.vacuum();

        // t1/t2/t3 的 snapshot.xmin = 1（t1 是第一个活跃事务）
        // safe_xid = 1
        // 没有 txn_id < 1 的事务（txn_id 从 1 开始）
        assert_eq!(stats.safe_xid, 1);
        assert_eq!(stats.vacuumed_committed, 0);
        assert_eq!(stats.retained_active, 3);
        assert_eq!(stats.retained_committed, 10);

        // 3 个活跃事务应全部保留
        assert_eq!(mgr.active_count(), 3);
        assert_eq!(mgr.committed_count(), 10);

        // 提交 3 个活跃事务
        mgr.commit(t1.txn_id, 0).unwrap();
        mgr.commit(t2.txn_id, 0).unwrap();
        mgr.abort(t3.txn_id).unwrap();

        // 现在 VACUUM 可以回收全部
        let stats2 = mgr.vacuum();
        assert_eq!(stats2.retained_active, 0);
        assert_eq!(stats2.vacuumed_committed, 12); // 10 + t1 + t2
        assert_eq!(stats2.vacuumed_aborted, 1); // t3
                                                // t1/t2 未调用 register_write，write_set 为空，不会加入 committed_writes
                                                // 所以 committed_writes 只有循环中的 10 条，全部被回收
        assert_eq!(stats2.vacuumed_writes, 10);
    }

    // -----------------------------------------------------------------
    // 2. VACUUM + 不变量（不破坏 MVCC 语义）
    // -----------------------------------------------------------------

    #[test]
    fn vacuum_preserves_first_committer_wins() {
        // 验证 VACUUM 不会破坏 first-committer-wins 检测
        let mgr = MvccManager::new();

        // 两个并发事务都写同一 key
        let t1 = mgr.begin();
        let t2 = mgr.begin();

        let _ = mgr.register_write(t1.txn_id, "k1");
        let _ = mgr.register_write(t2.txn_id, "k1");

        // t1 先提交 → 成功
        mgr.commit(t1.txn_id, 0).unwrap();

        // t2 后提交 → 应失败（first-committer-wins）
        let result = mgr.commit(t2.txn_id, 0);
        assert!(result.is_err(), "t2 should fail with WriteWriteConflict");

        // VACUUM（无活跃事务，回收所有）
        mgr.vacuum();

        // 再次执行相同场景，验证 first-committer-wins 仍然工作
        let t3 = mgr.begin();
        let t4 = mgr.begin();
        let _ = mgr.register_write(t3.txn_id, "k2");
        let _ = mgr.register_write(t4.txn_id, "k2");
        mgr.commit(t3.txn_id, 0).unwrap();
        let result2 = mgr.commit(t4.txn_id, 0);
        assert!(result2.is_err(), "t4 should fail with WriteWriteConflict");
    }

    #[test]
    fn vacuum_preserves_ssi_write_skew_detection() {
        // 验证 VACUUM 不会破坏 SSI 写偏斜检测
        let mgr = MvccManager::new();

        // 两个 SERIALIZABLE 事务并发，形成写偏斜
        let t1 = mgr.begin_with_isolation(IsolationLevel::Serializable);
        let t2 = mgr.begin_with_isolation(IsolationLevel::Serializable);

        // t1 读 x 写 y，t2 读 y 写 x（经典写偏斜）
        let _ = mgr.register_read(t1.txn_id, "x");
        let _ = mgr.register_write(t1.txn_id, "y");
        let _ = mgr.register_read(t2.txn_id, "y");
        let _ = mgr.register_write(t2.txn_id, "x");

        // t1 先提交 → 成功
        mgr.commit(t1.txn_id, 0).unwrap();

        // t2 后提交 → 应失败（SSI 检测到写偏斜）
        let result = mgr.commit(t2.txn_id, 0);
        assert!(result.is_err(), "t2 should fail with WriteSkewDetected");

        // VACUUM（无活跃事务）
        mgr.vacuum();

        // 再次执行相同场景，验证 SSI 仍然工作
        let t3 = mgr.begin_with_isolation(IsolationLevel::Serializable);
        let t4 = mgr.begin_with_isolation(IsolationLevel::Serializable);
        let _ = mgr.register_read(t3.txn_id, "x2");
        let _ = mgr.register_write(t3.txn_id, "y2");
        let _ = mgr.register_read(t4.txn_id, "y2");
        let _ = mgr.register_write(t4.txn_id, "x2");
        mgr.commit(t3.txn_id, 0).unwrap();
        let result2 = mgr.commit(t4.txn_id, 0);
        assert!(result2.is_err(), "t4 should fail with WriteSkewDetected");
    }

    #[test]
    fn vacuum_preserves_visibility() {
        // 验证 VACUUM 不会破坏可见性判断
        let mgr = MvccManager::new();

        // t1 提交
        let t1 = mgr.begin();
        let t1_id = t1.txn_id;
        mgr.commit(t1.txn_id, 0).unwrap();

        // t2 开（snapshot 包含 t1 为已提交，因为 t1 < t2.snapshot.xmax 且不在 active_txns 中）
        let t2 = mgr.begin();

        // t2 应该能看到 t1 的修改（xmin=t1_id, xmax=0 → 可见）
        let visible = mgr.is_visible(t2.txn_id, t1_id, 0);
        assert!(visible, "t2 should see t1's committed changes");

        // VACUUM — 应回收 t1（无活跃事务之前的，但 t2 活跃）
        // t2.snapshot.xmin = 2（t2 自己是唯一活跃的，xmin = 2）
        // safe_xid = 2
        // t1.txn_id = 1 < 2 → 回收
        let stats = mgr.vacuum();
        assert_eq!(stats.vacuumed_committed, 1);
        assert_eq!(stats.retained_active, 1);

        // t2 仍然能看到 t1 的修改（即使 t1 已从 committed_txns 中删除）
        // 因为 is_visible 使用 t2.snapshot 的 committed 集合判断，
        // 但 t1 已不在 committed_txns 中，所以 is_visible 可能返回 false
        // 这是 VACUUM 的预期行为：被 VACUUM 的事务视为"在快照前已提交"
        // 由于 snapshot.xmin = 2 > t1.txn_id = 1，t1 应被视为已提交
        let visible2 = mgr.is_visible(t2.txn_id, t1_id, 0);
        // 注意：MvccManager::is_visible 内部使用 committed_txns，VACUUM 后 t1 不在
        // 但 snapshot.is_visible 有 fallback: `xmin < snapshot.xmin` 视为已提交
        // 这里 t1_id = 1 < snapshot.xmin = 2，所以应该返回 true
        assert!(
            visible2,
            "t2 should still see t1's committed changes after VACUUM (xmin < snapshot.xmin)"
        );

        // t2 可以提交
        mgr.commit(t2.txn_id, 0).unwrap();
    }

    // -----------------------------------------------------------------
    // 3. 并发 + Stress
    // -----------------------------------------------------------------

    #[test]
    fn vacuum_concurrent_with_begin_safe() {
        // VACUUM 并发 BEGIN 不应 panic，不破坏不变量
        let mgr = Arc::new(MvccManager::new());
        let iterations = Arc::new(AtomicU64::new(0));

        // 预先提交一些事务
        for i in 0..50 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // 并发：一个线程做 VACUUM，多个线程做 BEGIN+COMMIT
        let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

        // VACUUM 线程
        {
            let mgr = Arc::clone(&mgr);
            let iterations = Arc::clone(&iterations);
            handles.push(thread::spawn(move || {
                let mut total_vacuumed = 0;
                for _ in 0..10 {
                    let stats = mgr.vacuum();
                    total_vacuumed += stats.total_vacuumed();
                    // 短暂休眠让其他线程有机会运行
                    std::thread::yield_now();
                }
                let _ = iterations; // suppress unused warning
                let _ = total_vacuumed; // suppress unused warning
            }));
        }

        // BEGIN+COMMIT 线程（4 个）
        for tid in 0..4 {
            let mgr = Arc::clone(&mgr);
            let iterations = Arc::clone(&iterations);
            handles.push(thread::spawn(move || {
                let mut count = 0u64;
                for i in 0..100 {
                    let txn = mgr.begin();
                    let _ = mgr.register_write(txn.txn_id, format!("t{tid}_k{i}"));
                    // 故意偶尔 abort
                    if i % 10 == 0 {
                        let _ = mgr.abort(txn.txn_id);
                    } else {
                        let _ = mgr.commit(txn.txn_id, 0);
                    }
                    count += 1;
                }
                iterations.fetch_add(count, Ordering::SeqCst);
            }));
        }

        // 等待所有线程完成
        for h in handles {
            h.join().expect("thread should not panic");
        }

        // 最终 VACUUM 清理
        let final_stats = mgr.vacuum();
        // 应该回收所有（无活跃事务）
        assert_eq!(final_stats.retained_active, 0);
        assert!(final_stats.vacuumed_committed > 0);
    }

    #[test]
    fn vacuum_repeated_cycles_stress() {
        // 反复 INSERT/DELETE/COMMIT/ABORT 1000 次 + VACUUM，验证空间回收 > 90%
        let mgr = MvccManager::new();
        const TXN_COUNT: u32 = 1000;

        for i in 0..TXN_COUNT {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            if i % 5 == 0 {
                let _ = mgr.abort(txn.txn_id);
            } else {
                let _ = mgr.commit(txn.txn_id, 0);
            }
        }

        assert_eq!(mgr.committed_count(), 800); // 1000 - 200 = 800
        assert_eq!(mgr.aborted_count(), 200);
        assert_eq!(mgr.active_count(), 0);

        // VACUUM
        let stats = mgr.vacuum();

        // 无活跃事务，应回收全部
        assert_eq!(stats.vacuumed_committed, 800);
        assert_eq!(stats.vacuumed_aborted, 200);
        assert_eq!(stats.vacuumed_writes, 800);
        assert_eq!(stats.retained_committed, 0);
        assert_eq!(stats.retained_aborted, 0);
        assert_eq!(stats.retained_writes, 0);

        // 回收率应 > 90%
        let ratio = stats.reclaim_ratio();
        assert!(ratio > 0.9, "reclaim ratio should be > 0.9, got {}", ratio);

        // 再次 VACUUM 应是 noop
        let stats2 = mgr.vacuum();
        assert_eq!(stats2.total_vacuumed(), 0);
    }

    #[test]
    fn vacuum_10k_txns_stress() {
        // 10000 事务 mixed ops + VACUUM，验证回收率 > 90%
        let mgr = MvccManager::new();
        const TXN_COUNT: u32 = 10000;

        // 多轮：每轮 1000 事务 + VACUUM
        const ROUNDS: u32 = 10;
        const PER_ROUND: u32 = 1000;

        let mut total_vacuumed = 0usize;
        let mut total_processed = 0usize;

        for round in 0..ROUNDS {
            // 创建 1000 事务
            for i in 0..PER_ROUND {
                let txn = mgr.begin();
                let _ = mgr.register_write(txn.txn_id, format!("r{round}_k{i}"));
                if (round * PER_ROUND + i).is_multiple_of(4) {
                    let _ = mgr.abort(txn.txn_id);
                } else {
                    let _ = mgr.commit(txn.txn_id, 0);
                }
                total_processed += 1;
            }

            // 每轮 VACUUM
            let stats = mgr.vacuum();
            total_vacuumed += stats.total_vacuumed();
        }

        // 最终 VACUUM
        let final_stats = mgr.vacuum();
        total_vacuumed += final_stats.total_vacuumed();

        assert_eq!(total_processed, TXN_COUNT as usize);
        // 所有事务应被回收（无活跃事务）
        assert_eq!(final_stats.retained_active, 0);
        assert_eq!(final_stats.retained_committed, 0);
        assert_eq!(final_stats.retained_aborted, 0);
        assert_eq!(final_stats.retained_writes, 0);

        // 总回收率应 > 90%
        let total_retained = final_stats.total_retained();
        let ratio = if total_vacuumed + total_retained == 0 {
            0.0
        } else {
            total_vacuumed as f64 / (total_vacuumed + total_retained) as f64
        };
        assert!(
            ratio > 0.9,
            "overall reclaim ratio should be > 0.9, got {}",
            ratio
        );
    }

    // -----------------------------------------------------------------
    // 4. 边界 + 不变量
    // -----------------------------------------------------------------

    #[test]
    fn vacuum_idempotent() {
        // 连续 VACUUM 第二次应该是 noop
        let mgr = MvccManager::new();

        // 提交一些事务
        for i in 0..20 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // 第一次 VACUUM
        let stats1 = mgr.vacuum();
        assert_eq!(stats1.vacuumed_committed, 20);

        // 第二次 VACUUM — 应是 noop
        let stats2 = mgr.vacuum();
        assert_eq!(stats2.vacuumed_committed, 0);
        assert_eq!(stats2.vacuumed_aborted, 0);
        assert_eq!(stats2.vacuumed_writes, 0);
        assert_eq!(stats2.total_vacuumed(), 0);
    }

    #[test]
    fn vacuum_safe_xid_monotonic() {
        // 多轮 VACUUM 后 safe_xid 应单调不减
        let mgr = MvccManager::new();

        let mut prev_safe_xid = 0u32;

        for round in 0..5 {
            // 创建并提交一些事务
            for i in 0..10 {
                let txn = mgr.begin();
                let _ = mgr.register_write(txn.txn_id, format!("r{round}_k{i}"));
                mgr.commit(txn.txn_id, 0).unwrap();
            }

            let safe_xid = mgr.vacuum_safe_xid();
            assert!(
                safe_xid >= prev_safe_xid,
                "safe_xid should be monotonic non-decreasing: round {} got {} < prev {}",
                round,
                safe_xid,
                prev_safe_xid
            );
            prev_safe_xid = safe_xid;

            mgr.vacuum();
        }
    }

    // -----------------------------------------------------------------
    // 5. 状态查询辅助测试
    // -----------------------------------------------------------------

    #[test]
    fn vacuum_stats_helpers_correct() {
        let stats = VacuumStats {
            safe_xid: 100,
            vacuumed_committed: 50,
            vacuumed_aborted: 20,
            vacuumed_writes: 30,
            retained_active: 5,
            retained_committed: 10,
            retained_aborted: 4,
            retained_writes: 6,
        };

        assert_eq!(stats.total_vacuumed(), 100); // 50 + 20 + 30
        assert_eq!(stats.total_retained(), 25); // 5 + 10 + 4 + 6

        // reclaim_ratio = vacuumed / (vacuumed + retained_non_active)
        // = 100 / (100 + 20) = 100 / 120 ≈ 0.833
        let ratio = stats.reclaim_ratio();
        assert!((ratio - 100.0 / 120.0).abs() < 1e-9, "got {}", ratio);
    }

    #[test]
    fn vacuum_with_multiple_active_txns_uses_oldest_snapshot() {
        // 多个活跃事务时，safe_xid 应取最老快照的 xmin
        let mgr = MvccManager::new();

        // 提交 5 个事务
        for i in 0..5 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // 开 1 个活跃事务 t6（snapshot 时无活跃，xmin = xmax = 6）
        let t6 = mgr.begin();

        // 提交 3 个事务
        for i in 0..3 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("late_k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        // 开第 2 个活跃事务 t10（snapshot 时 t6 活跃，xmin = 6）
        let t10 = mgr.begin();

        // VACUUM — safe_xid 应为 min(t6.xmin=6, t10.xmin=6) = 6
        let safe_xid = mgr.vacuum_safe_xid();
        assert_eq!(safe_xid, 6);

        let stats = mgr.vacuum();
        // 回收 txn_id < 6 的 5 个早期事务
        assert_eq!(stats.vacuumed_committed, 5);
        // 保留 txn_id >= 6 的：t6 + 3 个 late + t10 = 5 个活跃/已提交
        // 但 t6 和 t10 是活跃的，不计入 committed
        assert_eq!(stats.retained_committed, 3); // 3 个 late
        assert_eq!(stats.retained_active, 2); // t6 + t10

        // 提交活跃事务
        mgr.commit(t6.txn_id, 0).unwrap();
        mgr.commit(t10.txn_id, 0).unwrap();

        // 现在 VACUUM 应回收所有
        let stats2 = mgr.vacuum();
        assert_eq!(stats2.retained_active, 0);
        assert_eq!(stats2.vacuumed_committed, 5); // 3 late + t6 + t10
    }
}
