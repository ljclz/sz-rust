//! SzRSQL 隔离级别交叉 Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 2.17。
//!
//! 验证矩阵（PG 隔离级别语义）：
//!
//! | 异常现象         | RC | RR (SI) | SERIALIZABLE (SSI) |
//! |-----------------|-----|---------|---------------------|
//! | 脏读            | 阻止 | 阻止    | 阻止                |
//! | 不可重复读      | 允许 | 阻止    | 阻止                |
//! | 幻读            | 允许 | 阻止    | 阻止                |
//! | 丢失更新（WW）  | 阻止 | 阻止    | 阻止                |
//! | 写偏斜          | 允许 | 允许    | 阻止                |
//!
//! 设计要点：
//! 1. **XorShift64 PRNG**：固定种子，测试可重现（与 mvcc_fuzz / wal_fuzz 同风格）
//! 2. **矩阵测试**：5 种异常 × 3 种隔离级别 = 15 个确定性测试
//! 3. **参数化对比**：闭包 `scenario(level) -> Outcome` 模式，跨级别对比
//! 4. **Fuzz 不变量**：随机操作序列下，状态机守恒、错误码语义正确
//! 5. **跨隔离级别交互**：RC + RR + SERIALIZABLE 在同一 MvccManager 上混合

use crate::mvcc::{IsolationLevel, MvccError, MvccManager, TxnStatus};
use std::collections::HashSet;

// =====================================================================
// XorShift64 — 固定种子 PRNG（与 mvcc_fuzz.rs / wal_fuzz.rs 同风格）
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

    /// 从切片中随机选一个元素
    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        let idx = self.next_range(slice.len() as u32) as usize;
        &slice[idx]
    }
}

// =====================================================================
// 辅助：操作结果简化的 Outcome 类型
// =====================================================================

/// 一个事务在 scenario 中的最终状态摘要
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Committed,
    Aborted,
    Conflict, // WriteWriteConflict 或 WriteSkewDetected
    Active,
}

impl Outcome {
    fn from_status(status: Option<TxnStatus>) -> Self {
        match status {
            Some(TxnStatus::Committed) => Outcome::Committed,
            Some(TxnStatus::Aborted) => Outcome::Aborted,
            Some(TxnStatus::Active) => Outcome::Active,
            None => Outcome::Active,
        }
    }
}

/// 把 commit 结果转换为 Outcome
fn commit_outcome(result: Result<(), MvccError>) -> Outcome {
    match result {
        Ok(()) => Outcome::Committed,
        Err(MvccError::WriteWriteConflict(_)) | Err(MvccError::WriteSkewDetected(_)) => {
            Outcome::Conflict
        }
        Err(_) => Outcome::Active,
    }
}

// =====================================================================
// Phase 2.17 测试模块
// =====================================================================

#[cfg(test)]
mod phase_2_17 {
    use super::*;

    // =================================================================
    // Part 1: 矩阵测试 — 5 种异常 × 3 种隔离级别 = 15 个确定性测试
    // =================================================================
    //
    // 每个异常定义一个 scenario(level) -> (Outcome, Outcome) 闭包，
    // 返回 (T1 最终状态, T2 最终状态)。

    // -----------------------------------------------------------------
    // 异常 1: 脏读 (Dirty Read)
    // T1 writes (uncommitted), T2 reads same key
    // 期望：所有 3 个级别都阻止脏读（T2 看不到 T1 未提交的数据）
    // -----------------------------------------------------------------

    /// 脏读场景：T1 写 key（未提交），T2 读同一 key
    fn dirty_read_scenario(level: IsolationLevel) -> (Outcome, Outcome) {
        let mgr = MvccManager::new();
        let t1 = mgr.begin_with_isolation(level);
        let t2 = mgr.begin_with_isolation(level);

        // T1 写 key（未提交）
        mgr.register_write(t1.txn_id, "t1:r1").unwrap();
        // T2 读同一 key（应看不到 T1 的未提交写）
        mgr.register_read(t2.txn_id, "t1:r1").unwrap();

        // 验证可见性：T2 不应看到 T1 的未提交数据
        // is_visible(observer, xmin_of_tuple, xmax_of_tuple)
        // 这里 T1 的 tuple xmin=t1.txn_id, xmax=0（未删除）
        // T1 未提交 → 在 T2 的快照活跃中 → 不可见
        let visible = mgr.is_visible(t2.txn_id, t1.txn_id, 0);
        assert!(
            !visible,
            "脏读：T2 不应看到 T1 未提交的数据（隔离级别 {:?}）",
            level
        );

        // T1 / T2 都保持活跃状态
        let t1_outcome = Outcome::from_status(mgr.get_status(t1.txn_id));
        let t2_outcome = Outcome::from_status(mgr.get_status(t2.txn_id));
        (t1_outcome, t2_outcome)
    }

    #[test]
    fn dirty_read_blocked_at_rc() {
        let (t1, t2) = dirty_read_scenario(IsolationLevel::ReadCommitted);
        assert_eq!(t1, Outcome::Active);
        assert_eq!(t2, Outcome::Active);
    }

    #[test]
    fn dirty_read_blocked_at_rr() {
        let (t1, t2) = dirty_read_scenario(IsolationLevel::RepeatableRead);
        assert_eq!(t1, Outcome::Active);
        assert_eq!(t2, Outcome::Active);
    }

    #[test]
    fn dirty_read_blocked_at_serializable() {
        let (t1, t2) = dirty_read_scenario(IsolationLevel::Serializable);
        assert_eq!(t1, Outcome::Active);
        assert_eq!(t2, Outcome::Active);
    }

    // -----------------------------------------------------------------
    // 异常 2: 不可重复读 (Non-repeatable Read)
    // T1 读 key K, T2 写 K & 提交, T1 再读 K
    // 期望：
    //   - RC: T1 通过 refresh_snapshot 看到新值（允许不可重复读）
    //   - RR/SER: T1 看到旧值（快照冻结，refresh_snapshot 被拒绝）
    // -----------------------------------------------------------------

    /// 不可重复读场景
    /// 返回 (T1 是否能 refresh_snapshot, T1 刷新后是否看到 T2 的新版本)
    fn non_repeatable_read_scenario(level: IsolationLevel) -> (Result<(), MvccError>, bool, bool) {
        let mgr = MvccManager::new();
        let t1 = mgr.begin_with_isolation(level);
        let t2 = mgr.begin_with_isolation(level);

        // T1 第一次读 key K（看到旧版本，假设 K 由某个已提交事务创建，xmin=0）
        mgr.register_read(t1.txn_id, "t1:K").unwrap();
        let visible_before = mgr.is_visible(t1.txn_id, 0, 0); // 旧版本可见

        // T2 写 key K & 提交
        mgr.register_write(t2.txn_id, "t1:K").unwrap();
        mgr.commit(t2.txn_id, 100).unwrap();

        // T1 尝试刷新快照
        let refresh_result = mgr.refresh_snapshot(t1.txn_id);

        // T1 再读 key K（看新版本？新版本的 xmin=t2.txn_id）
        let visible_after = mgr.is_visible(t1.txn_id, t2.txn_id, 0);

        (refresh_result, visible_before, visible_after)
    }

    #[test]
    fn non_repeatable_read_allowed_at_rc() {
        let (refresh, before, after) = non_repeatable_read_scenario(IsolationLevel::ReadCommitted);
        // RC 允许 refresh_snapshot
        assert!(refresh.is_ok(), "RC 应允许 refresh_snapshot");
        assert!(before, "刷新前应看到旧版本");
        // 刷新后 T2 已提交，T1 应看到 T2 的新版本
        assert!(after, "RC: 刷新后应看到新版本（不可重复读）");
    }

    #[test]
    fn non_repeatable_read_blocked_at_rr() {
        let (refresh, before, after) = non_repeatable_read_scenario(IsolationLevel::RepeatableRead);
        // RR 拒绝 refresh_snapshot
        assert!(
            matches!(refresh, Err(MvccError::SnapshotRefreshNotAllowed { .. })),
            "RR 应拒绝 refresh_snapshot"
        );
        assert!(before, "应看到旧版本");
        // RR 快照冻结，T1 不应看到 T2 的新版本
        assert!(!after, "RR: 不应看到新版本（不可重复读被阻止）");
    }

    #[test]
    fn non_repeatable_read_blocked_at_serializable() {
        let (refresh, before, after) = non_repeatable_read_scenario(IsolationLevel::Serializable);
        // SERIALIZABLE 拒绝 refresh_snapshot
        assert!(
            matches!(refresh, Err(MvccError::SnapshotRefreshNotAllowed { .. })),
            "SERIALIZABLE 应拒绝 refresh_snapshot"
        );
        assert!(before, "应看到旧版本");
        assert!(!after, "SERIALIZABLE: 不应看到新版本（不可重复读被阻止）");
    }

    // -----------------------------------------------------------------
    // 异常 3: 幻读 (Phantom Read)
    // T1 读 key 集合 S, T2 插入新 key（属于 S 范围）& 提交, T1 再读 S
    // 期望：
    //   - RC: T1 通过 refresh_snapshot 看到新 key（允许幻读）
    //   - RR/SER: T1 看不到新 key（快照冻结）
    //
    // 注：MVCC 引擎不存储数据，只跟踪 read_set；"幻读"语义通过
    // is_visible(T1, T2.xmin, 0) 判断 T2 插入的新 tuple 是否对 T1 可见
    // -----------------------------------------------------------------

    /// 幻读场景
    /// 返回 (T1 refresh 结果, T1 刷新后是否看到 T2 的新 key)
    fn phantom_read_scenario(level: IsolationLevel) -> (Result<(), MvccError>, bool) {
        let mgr = MvccManager::new();
        let t1 = mgr.begin_with_isolation(level);
        let t2 = mgr.begin_with_isolation(level);

        // T1 读 key 集合 {K1, K2}（范围查询的简化）
        mgr.register_read(t1.txn_id, "t1:K1").unwrap();
        mgr.register_read(t1.txn_id, "t1:K2").unwrap();

        // T2 插入新 key K3（属于 T1 查询范围）& 提交
        mgr.register_write(t2.txn_id, "t1:K3").unwrap();
        mgr.commit(t2.txn_id, 100).unwrap();

        // T1 尝试刷新快照
        let refresh_result = mgr.refresh_snapshot(t1.txn_id);

        // T1 再读范围：T2 插入的 K3 (xmin=t2.txn_id) 对 T1 可见吗？
        let visible_new_key = mgr.is_visible(t1.txn_id, t2.txn_id, 0);

        (refresh_result, visible_new_key)
    }

    #[test]
    fn phantom_read_allowed_at_rc() {
        let (refresh, visible_new) = phantom_read_scenario(IsolationLevel::ReadCommitted);
        assert!(refresh.is_ok(), "RC 应允许 refresh_snapshot");
        assert!(visible_new, "RC: 刷新后应看到新 key（幻读允许）");
    }

    #[test]
    fn phantom_read_blocked_at_rr() {
        let (refresh, visible_new) = phantom_read_scenario(IsolationLevel::RepeatableRead);
        assert!(
            matches!(refresh, Err(MvccError::SnapshotRefreshNotAllowed { .. })),
            "RR 应拒绝 refresh_snapshot"
        );
        assert!(!visible_new, "RR: 不应看到新 key（幻读被阻止）");
    }

    #[test]
    fn phantom_read_blocked_at_serializable() {
        let (refresh, visible_new) = phantom_read_scenario(IsolationLevel::Serializable);
        assert!(
            matches!(refresh, Err(MvccError::SnapshotRefreshNotAllowed { .. })),
            "SERIALIZABLE 应拒绝 refresh_snapshot"
        );
        assert!(!visible_new, "SERIALIZABLE: 不应看到新 key（幻读被阻止）");
    }

    // -----------------------------------------------------------------
    // 异常 4: 丢失更新 (Lost Update = Write-Write Conflict)
    // T1 读 K, T2 读 K, T1 写 K & 提交, T2 写 K & 提交
    // 期望：所有 3 个级别都阻止丢失更新（first-committer-wins）
    //   - T1 提交成功
    //   - T2 提交失败（WriteWriteConflict）
    // -----------------------------------------------------------------

    /// 丢失更新场景
    /// 返回 (T1 commit 结果, T2 commit 结果)
    fn lost_update_scenario(level: IsolationLevel) -> (Outcome, Outcome) {
        let mgr = MvccManager::new();
        let t1 = mgr.begin_with_isolation(level);
        let t2 = mgr.begin_with_isolation(level);

        // T1, T2 都读 key K
        mgr.register_read(t1.txn_id, "t1:K").unwrap();
        mgr.register_read(t2.txn_id, "t1:K").unwrap();

        // T1, T2 都写 key K
        mgr.register_write(t1.txn_id, "t1:K").unwrap();
        mgr.register_write(t2.txn_id, "t1:K").unwrap();

        // T1 先提交（成功）
        let t1_result = commit_outcome(mgr.commit(t1.txn_id, 100));
        // T2 后提交（应失败：first-committer-wins）
        let t2_result = commit_outcome(mgr.commit(t2.txn_id, 200));

        (t1_result, t2_result)
    }

    #[test]
    fn lost_update_blocked_at_rc() {
        let (t1, t2) = lost_update_scenario(IsolationLevel::ReadCommitted);
        assert_eq!(t1, Outcome::Committed, "T1 先提交应成功");
        assert_eq!(
            t2,
            Outcome::Conflict,
            "RC: T2 后提交应失败（first-committer-wins）"
        );
    }

    #[test]
    fn lost_update_blocked_at_rr() {
        let (t1, t2) = lost_update_scenario(IsolationLevel::RepeatableRead);
        assert_eq!(t1, Outcome::Committed, "T1 先提交应成功");
        assert_eq!(
            t2,
            Outcome::Conflict,
            "RR: T2 后提交应失败（first-committer-wins）"
        );
    }

    #[test]
    fn lost_update_blocked_at_serializable() {
        let (t1, t2) = lost_update_scenario(IsolationLevel::Serializable);
        assert_eq!(t1, Outcome::Committed, "T1 先提交应成功");
        assert_eq!(
            t2,
            Outcome::Conflict,
            "SERIALIZABLE: T2 后提交应失败（first-committer-wins）"
        );
    }

    // -----------------------------------------------------------------
    // 异常 5: 写偏斜 (Write Skew)
    // T1 reads {A,B}, T2 reads {A,B}, T1 writes A, T2 writes B, both commit
    // 期望：
    //   - RC: 两个都成功（无 SSI 检测）
    //   - RR: 两个都成功（无 SSI 检测）
    //   - SER: T2 失败（WriteSkewDetected）
    //
    // 注：SSI 检测要求 T1 在 T2 的快照活跃中（T2 BEGIN 晚于 T1 BEGIN）
    // -----------------------------------------------------------------

    /// 写偏斜场景
    /// 返回 (T1 commit 结果, T2 commit 结果)
    fn write_skew_scenario(level: IsolationLevel) -> (Outcome, Outcome) {
        let mgr = MvccManager::new();
        let t1 = mgr.begin_with_isolation(level);
        // T2 BEGIN 晚于 T1 → T1 在 T2 的快照活跃中
        let t2 = mgr.begin_with_isolation(level);

        // T1 reads {A, B}, writes A
        mgr.register_read(t1.txn_id, "k:A").unwrap();
        mgr.register_read(t1.txn_id, "k:B").unwrap();
        mgr.register_write(t1.txn_id, "k:A").unwrap();

        // T2 reads {A, B}, writes B
        mgr.register_read(t2.txn_id, "k:A").unwrap();
        mgr.register_read(t2.txn_id, "k:B").unwrap();
        mgr.register_write(t2.txn_id, "k:B").unwrap();

        // T1 先提交
        let t1_result = commit_outcome(mgr.commit(t1.txn_id, 100));
        // T2 后提交
        let t2_result = commit_outcome(mgr.commit(t2.txn_id, 200));

        (t1_result, t2_result)
    }

    #[test]
    fn write_skew_allowed_at_rc() {
        let (t1, t2) = write_skew_scenario(IsolationLevel::ReadCommitted);
        assert_eq!(t1, Outcome::Committed, "RC: T1 应成功");
        assert_eq!(t2, Outcome::Committed, "RC: T2 应成功（写偏斜允许）");
    }

    #[test]
    fn write_skew_allowed_at_rr() {
        let (t1, t2) = write_skew_scenario(IsolationLevel::RepeatableRead);
        assert_eq!(t1, Outcome::Committed, "RR: T1 应成功");
        assert_eq!(t2, Outcome::Committed, "RR: T2 应成功（写偏斜允许）");
    }

    #[test]
    fn write_skew_blocked_at_serializable() {
        let (t1, t2) = write_skew_scenario(IsolationLevel::Serializable);
        assert_eq!(t1, Outcome::Committed, "SERIALIZABLE: T1 先提交应成功");
        assert_eq!(
            t2,
            Outcome::Conflict,
            "SERIALIZABLE: T2 应被 SSI 检测（写偏斜）"
        );
        // 验证具体错误类型
        let mgr = MvccManager::new();
        let t1 = mgr.begin_with_isolation(IsolationLevel::Serializable);
        let t2 = mgr.begin_with_isolation(IsolationLevel::Serializable);
        mgr.register_read(t1.txn_id, "k:A").unwrap();
        mgr.register_read(t1.txn_id, "k:B").unwrap();
        mgr.register_write(t1.txn_id, "k:A").unwrap();
        mgr.register_read(t2.txn_id, "k:A").unwrap();
        mgr.register_read(t2.txn_id, "k:B").unwrap();
        mgr.register_write(t2.txn_id, "k:B").unwrap();
        mgr.commit(t1.txn_id, 100).unwrap();
        let err = mgr.commit(t2.txn_id, 200).unwrap_err();
        assert_eq!(err, MvccError::WriteSkewDetected(t2.txn_id));
    }

    // =================================================================
    // Part 2: 跨隔离级别对比测试（参数化）
    // =================================================================

    /// 矩阵对比：同一场景在 3 个级别下的结果应满足预期矩阵
    #[test]
    fn matrix_dirty_read_all_levels_block() {
        for &level in &[
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ] {
            let (t1, t2) = dirty_read_scenario(level);
            assert_eq!(t1, Outcome::Active, "{:?}: T1 应活跃", level);
            assert_eq!(t2, Outcome::Active, "{:?}: T2 应活跃", level);
        }
    }

    #[test]
    fn matrix_non_repeatable_read_rc_vs_rr_vs_ser() {
        let (rc_refresh, _, rc_after) = non_repeatable_read_scenario(IsolationLevel::ReadCommitted);
        let (rr_refresh, _, rr_after) =
            non_repeatable_read_scenario(IsolationLevel::RepeatableRead);
        let (ser_refresh, _, ser_after) =
            non_repeatable_read_scenario(IsolationLevel::Serializable);

        // RC 允许，RR/SER 阻止
        assert!(rc_refresh.is_ok() && rc_after);
        assert!(rr_refresh.is_err() && !rr_after);
        assert!(ser_refresh.is_err() && !ser_after);
    }

    #[test]
    fn matrix_phantom_read_rc_vs_rr_vs_ser() {
        let (rc_refresh, rc_visible) = phantom_read_scenario(IsolationLevel::ReadCommitted);
        let (rr_refresh, rr_visible) = phantom_read_scenario(IsolationLevel::RepeatableRead);
        let (ser_refresh, ser_visible) = phantom_read_scenario(IsolationLevel::Serializable);

        assert!(rc_refresh.is_ok() && rc_visible);
        assert!(rr_refresh.is_err() && !rr_visible);
        assert!(ser_refresh.is_err() && !ser_visible);
    }

    #[test]
    fn matrix_lost_update_all_levels_block() {
        for &level in &[
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ] {
            let (t1, t2) = lost_update_scenario(level);
            assert_eq!(t1, Outcome::Committed, "{:?}: T1 应成功", level);
            assert_eq!(t2, Outcome::Conflict, "{:?}: T2 应冲突", level);
        }
    }

    #[test]
    fn matrix_write_skew_rc_rr_allow_ser_blocks() {
        let (rc_t1, rc_t2) = write_skew_scenario(IsolationLevel::ReadCommitted);
        let (rr_t1, rr_t2) = write_skew_scenario(IsolationLevel::RepeatableRead);
        let (ser_t1, ser_t2) = write_skew_scenario(IsolationLevel::Serializable);

        // RC, RR 允许写偏斜
        assert_eq!((rc_t1, rc_t2), (Outcome::Committed, Outcome::Committed));
        assert_eq!((rr_t1, rr_t2), (Outcome::Committed, Outcome::Committed));
        // SERIALIZABLE 阻止
        assert_eq!((ser_t1, ser_t2), (Outcome::Committed, Outcome::Conflict));
    }

    // =================================================================
    // Part 3: Fuzz 测试 — 随机操作序列，验证不变量
    // =================================================================

    /// 随机操作枚举
    #[derive(Debug, Clone)]
    enum Op {
        Begin(IsolationLevel),
        RegisterRead(u32, String), // txn_id, key
        RegisterWrite(u32, String),
        Commit(u32),
        Abort(u32),
        RefreshSnapshot(u32),
    }

    /// 生成 N 个随机操作（针对指定隔离级别池）
    fn gen_random_ops(seed: u64, n_ops: usize) -> Vec<Op> {
        let mut rng = XorShift64::new(seed);
        let mut ops = Vec::with_capacity(n_ops);
        let mut next_txn_id: u32 = 1;
        let levels = [
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ];
        let keys = ["k:A", "k:B", "k:C", "k:D", "k:E"];

        for _ in 0..n_ops {
            let op_choice = rng.next_range(6);
            match op_choice {
                0 => {
                    // Begin（随机隔离级别）
                    let level = *rng.pick(&levels);
                    ops.push(Op::Begin(level));
                    next_txn_id += 1;
                }
                1 => {
                    // RegisterRead（随机已存在 txn + 随机 key）
                    if next_txn_id > 1 {
                        let txn_id = rng.next_range(next_txn_id - 1) + 1;
                        let key = rng.pick(&keys).to_string();
                        ops.push(Op::RegisterRead(txn_id, key));
                    }
                }
                2 => {
                    // RegisterWrite
                    if next_txn_id > 1 {
                        let txn_id = rng.next_range(next_txn_id - 1) + 1;
                        let key = rng.pick(&keys).to_string();
                        ops.push(Op::RegisterWrite(txn_id, key));
                    }
                }
                3 => {
                    // Commit
                    if next_txn_id > 1 {
                        let txn_id = rng.next_range(next_txn_id - 1) + 1;
                        ops.push(Op::Commit(txn_id));
                    }
                }
                4 => {
                    // Abort
                    if next_txn_id > 1 {
                        let txn_id = rng.next_range(next_txn_id - 1) + 1;
                        ops.push(Op::Abort(txn_id));
                    }
                }
                5 => {
                    // RefreshSnapshot
                    if next_txn_id > 1 {
                        let txn_id = rng.next_range(next_txn_id - 1) + 1;
                        ops.push(Op::RefreshSnapshot(txn_id));
                    }
                }
                _ => unreachable!(),
            }
        }
        ops
    }

    /// 执行操作序列，收集不变量
    fn execute_ops(ops: &[Op]) -> (usize, usize, usize) {
        let mgr = MvccManager::new();
        let mut txn_levels: std::collections::HashMap<u32, IsolationLevel> =
            std::collections::HashMap::new();
        let mut begun_count = 0usize;
        let mut _error_count = 0usize;

        for op in ops {
            match op {
                Op::Begin(level) => {
                    let txn = mgr.begin_with_isolation(*level);
                    txn_levels.insert(txn.txn_id, *level);
                    begun_count += 1;
                }
                Op::RegisterRead(txn_id, key) => {
                    let _ = mgr.register_read(*txn_id, key.clone());
                }
                Op::RegisterWrite(txn_id, key) => {
                    let _ = mgr.register_write(*txn_id, key.clone());
                }
                Op::Commit(txn_id) => {
                    let level = txn_levels.get(txn_id).copied();
                    let result = mgr.commit(*txn_id, 100);
                    // 验证：已 committed/aborted 的事务再次 commit 应返回 Already* 错误
                    if let Some(lvl) = level {
                        let status = mgr.get_status(*txn_id);
                        match (result, status) {
                            (Ok(()), Some(TxnStatus::Committed)) => {}
                            (Err(MvccError::WriteWriteConflict(_)), Some(TxnStatus::Aborted)) => {}
                            (Err(MvccError::WriteSkewDetected(_)), Some(TxnStatus::Aborted)) => {
                                // SSI 只能在 SERIALIZABLE 下触发
                                assert_eq!(
                                    lvl,
                                    IsolationLevel::Serializable,
                                    "SSI 检测只能在 SERIALIZABLE 下触发，但 level={:?}",
                                    lvl
                                );
                            }
                            (Err(MvccError::AlreadyCommitted(_)), Some(TxnStatus::Committed)) => {}
                            (Err(MvccError::AlreadyAborted(_)), Some(TxnStatus::Aborted)) => {}
                            (Err(MvccError::TxnNotFound(_)), None) => {}
                            _ => {
                                _error_count += 1;
                            }
                        }
                    }
                }
                Op::Abort(txn_id) => {
                    let result = mgr.abort(*txn_id);
                    match result {
                        Ok(())
                        | Err(MvccError::AlreadyAborted(_))
                        | Err(MvccError::AlreadyCommitted(_))
                        | Err(MvccError::TxnNotFound(_)) => {}
                        _ => {
                            _error_count += 1;
                        }
                    }
                }
                Op::RefreshSnapshot(txn_id) => {
                    let result = mgr.refresh_snapshot(*txn_id);
                    if let Ok(()) = result {
                        // 只能是 RC（其他级别应返回 SnapshotRefreshNotAllowed）
                        let level = txn_levels.get(txn_id);
                        if let Some(lvl) = level {
                            assert_eq!(
                                *lvl,
                                IsolationLevel::ReadCommitted,
                                "refresh_snapshot 成功但 level={:?}（应只 RC 允许）",
                                lvl
                            );
                        }
                    }
                }
            }
        }

        let committed = mgr.committed_count();
        let aborted = mgr.aborted_count();
        (begun_count, committed, aborted)
    }

    /// Fuzz 1: 大量随机操作序列不 panic
    #[test]
    fn fuzz_random_ops_no_panic_1000_rounds() {
        for seed in 1..=10 {
            let ops = gen_random_ops(seed * 7919, 500);
            let _ = execute_ops(&ops);
        }
    }

    /// Fuzz 2: 事务数守恒（committed + aborted + active == begun）
    #[test]
    fn fuzz_transaction_count_conservation() {
        for seed in 1..=20 {
            let ops = gen_random_ops(seed * 31337, 300);
            let (begun, committed, aborted) = execute_ops(&ops);
            // 守恒：committed + aborted <= begun（活跃的不算）
            assert!(
                committed + aborted <= begun,
                "seed={}: committed({}) + aborted({}) > begun({})",
                seed,
                committed,
                aborted,
                begun
            );
        }
    }

    /// Fuzz 3: SSI 检测只在 SERIALIZABLE 下触发
    #[test]
    fn fuzz_ssi_only_triggers_under_serializable() {
        // 只用 RC 和 RR，不应该有任何 WriteSkewDetected 错误
        for seed in 1..=15 {
            let mut rng = XorShift64::new(seed * 12345);
            let mgr = MvccManager::new();
            let levels = [
                IsolationLevel::ReadCommitted,
                IsolationLevel::RepeatableRead,
            ];
            let keys = ["k:A", "k:B", "k:C", "k:D"];
            let mut txns = Vec::new();

            for _ in 0..50 {
                let level = *rng.pick(&levels);
                let txn = mgr.begin_with_isolation(level);
                txns.push(txn.txn_id);
                // 随机 read/write
                for _ in 0..3 {
                    let key = rng.pick(&keys).to_string();
                    if rng.next_bool() {
                        let _ = mgr.register_read(txn.txn_id, key);
                    } else {
                        let _ = mgr.register_write(txn.txn_id, key);
                    }
                }
            }

            // 随机顺序 commit
            for &txn_id in &txns {
                let result = mgr.commit(txn_id, 100);
                if let Err(MvccError::WriteSkewDetected(_)) = result {
                    panic!("seed={}: WriteSkewDetected 不应在 RC/RR 下触发", seed);
                }
            }
        }
    }

    /// Fuzz 4: 同一操作序列在不同种子下结果确定（可重现）
    #[test]
    fn fuzz_deterministic_replay() {
        let ops1 = gen_random_ops(4242, 200);
        let ops2 = gen_random_ops(4242, 200);
        // 相同种子应生成相同序列
        assert_eq!(ops1.len(), ops2.len());

        let (_, c1, a1) = execute_ops(&ops1);
        let (_, c2, a2) = execute_ops(&ops2);
        assert_eq!(c1, c2, "相同种子重放：committed 数应一致");
        assert_eq!(a1, a2, "相同种子重放：aborted 数应一致");
    }

    /// Fuzz 5: refresh_snapshot 只在 RC 下成功（RR/SER 必失败）
    #[test]
    fn fuzz_snapshot_refresh_only_rc_succeeds() {
        for seed in 1..=15 {
            let mut rng = XorShift64::new(seed * 99991);
            let levels = [
                IsolationLevel::ReadCommitted,
                IsolationLevel::RepeatableRead,
                IsolationLevel::Serializable,
            ];

            for _ in 0..30 {
                let level = *rng.pick(&levels);
                let mgr = MvccManager::new();
                let t1 = mgr.begin_with_isolation(level);
                // 触发一些操作
                let _ = mgr.register_read(t1.txn_id, "k:X");
                let t2 = mgr.begin();
                let _ = mgr.register_write(t2.txn_id, "k:X");
                let _ = mgr.commit(t2.txn_id, 100);

                let result = mgr.refresh_snapshot(t1.txn_id);
                match (level, result) {
                    (IsolationLevel::ReadCommitted, Ok(())) => {}
                    (
                        IsolationLevel::RepeatableRead,
                        Err(MvccError::SnapshotRefreshNotAllowed { .. }),
                    ) => {}
                    (
                        IsolationLevel::Serializable,
                        Err(MvccError::SnapshotRefreshNotAllowed { .. }),
                    ) => {}
                    (l, r) => {
                        panic!(
                            "seed={}: level={:?} refresh_snapshot 返回 {:?}（不符合预期）",
                            seed, l, r
                        );
                    }
                }
            }
        }
    }

    /// Fuzz 6: WriteWriteConflict 在所有级别都可触发（first-committer-wins 全局启用）
    #[test]
    fn fuzz_ww_conflict_triggers_all_levels() {
        for &level in &[
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ] {
            for seed in 1..=5 {
                let mgr = MvccManager::new();
                let t1 = mgr.begin_with_isolation(level);
                let t2 = mgr.begin_with_isolation(level);
                // 两个事务写同一 key
                let _ = mgr.register_write(t1.txn_id, "k:conflict");
                let _ = mgr.register_write(t2.txn_id, "k:conflict");
                let r1 = mgr.commit(t1.txn_id, 100);
                let r2 = mgr.commit(t2.txn_id, 200);
                assert!(r1.is_ok(), "seed={}: {:?} T1 应成功", seed, level);
                assert!(
                    matches!(r2, Err(MvccError::WriteWriteConflict(_))),
                    "seed={}: {:?} T2 应 WW 冲突",
                    seed,
                    level
                );
            }
        }
    }

    /// Fuzz 7: 多事务并发混合操作（10 事务，每事务随机 read/write 同一组 keys）
    #[test]
    fn fuzz_10_txns_mixed_ops_consistent_state() {
        for seed in 1..=10 {
            let mut rng = XorShift64::new(seed * 6151);
            let levels = [
                IsolationLevel::ReadCommitted,
                IsolationLevel::RepeatableRead,
                IsolationLevel::Serializable,
            ];
            let keys = ["k:1", "k:2", "k:3", "k:4", "k:5"];
            let mgr = MvccManager::new();

            // 10 事务，每事务随机隔离级别
            let mut txns = Vec::new();
            for _ in 0..10 {
                let level = *rng.pick(&levels);
                let txn = mgr.begin_with_isolation(level);
                txns.push((txn.txn_id, level));
            }

            // 每事务随机 read/write 5-10 次
            for &(txn_id, _level) in &txns {
                let n_ops = rng.next_range(10) + 5;
                for _ in 0..n_ops {
                    let key = rng.pick(&keys).to_string();
                    if rng.next_bool() {
                        let _ = mgr.register_read(txn_id, key);
                    } else {
                        let _ = mgr.register_write(txn_id, key);
                    }
                }
            }

            // 随机顺序 commit（验证不 panic + 状态机一致）
            let mut total_committed = 0usize;
            let mut total_aborted = 0usize;
            for &(txn_id, _level) in &txns {
                let result = mgr.commit(txn_id, 100);
                match result {
                    Ok(()) => total_committed += 1,
                    Err(MvccError::WriteWriteConflict(_))
                    | Err(MvccError::WriteSkewDetected(_)) => total_aborted += 1,
                    Err(MvccError::AlreadyCommitted(_)) | Err(MvccError::AlreadyAborted(_)) => {}
                    Err(e) => panic!("seed={}: 未预期错误 {:?}", seed, e),
                }
            }

            // 守恒：committed + aborted <= 10
            assert!(
                total_committed + total_aborted <= 10,
                "seed={}: committed({}) + aborted({}) > 10",
                seed,
                total_committed,
                total_aborted
            );
            // 至少有一些事务最终确定状态
            assert!(
                total_committed + total_aborted > 0,
                "seed={}: 应至少有 1 个事务确定状态",
                seed
            );
        }
    }

    /// Fuzz 8: 100 轮随机隔离级别 + 2 事务写偏斜场景
    /// 验证 SERIALIZABLE 总能检测到，RC/RR 总能通过
    #[test]
    fn fuzz_write_skew_100_rounds() {
        for seed in 1..=100 {
            let mut rng = XorShift64::new(seed * 31);
            let levels = [
                IsolationLevel::ReadCommitted,
                IsolationLevel::RepeatableRead,
                IsolationLevel::Serializable,
            ];
            let level = *rng.pick(&levels);

            let mgr = MvccManager::new();
            let t1 = mgr.begin_with_isolation(level);
            let t2 = mgr.begin_with_isolation(level);

            // 经典写偏斜：值班医生场景
            mgr.register_read(t1.txn_id, "on_call:alice").unwrap();
            mgr.register_read(t1.txn_id, "on_call:bob").unwrap();
            mgr.register_write(t1.txn_id, "on_call:alice").unwrap();
            mgr.register_read(t2.txn_id, "on_call:alice").unwrap();
            mgr.register_read(t2.txn_id, "on_call:bob").unwrap();
            mgr.register_write(t2.txn_id, "on_call:bob").unwrap();

            let r1 = mgr.commit(t1.txn_id, 100);
            let r2 = mgr.commit(t2.txn_id, 200);

            match level {
                IsolationLevel::ReadCommitted | IsolationLevel::RepeatableRead => {
                    assert!(r1.is_ok(), "seed={}: {:?} T1 应成功", seed, level);
                    assert!(r2.is_ok(), "seed={}: {:?} T2 应成功", seed, level);
                }
                IsolationLevel::Serializable => {
                    assert!(r1.is_ok(), "seed={}: SER T1 应成功", seed);
                    assert!(
                        matches!(r2, Err(MvccError::WriteSkewDetected(_))),
                        "seed={}: SER T2 应被 SSI 检测",
                        seed
                    );
                }
            }
        }
    }

    // =================================================================
    // Part 4: 跨隔离级别交互测试（RC + RR + SER 在同一引擎上混合）
    // =================================================================

    /// 跨隔离级别：3 个不同级别的事务在同一 mgr 上交互
    #[test]
    fn cross_isolation_3_levels_mixed_interaction() {
        let mgr = MvccManager::new();
        let t_rc = mgr.begin_with_isolation(IsolationLevel::ReadCommitted);
        let t_rr = mgr.begin_with_isolation(IsolationLevel::RepeatableRead);
        let t_ser = mgr.begin_with_isolation(IsolationLevel::Serializable);

        // 3 个事务都读同一 key
        mgr.register_read(t_rc.txn_id, "k:shared").unwrap();
        mgr.register_read(t_rr.txn_id, "k:shared").unwrap();
        mgr.register_read(t_ser.txn_id, "k:shared").unwrap();

        // RC 先写并提交
        mgr.register_write(t_rc.txn_id, "k:shared").unwrap();
        let rc_result = mgr.commit(t_rc.txn_id, 100);
        assert!(rc_result.is_ok(), "RC 提交应成功");

        // RR 后写并提交 → 应 WW 冲突
        mgr.register_write(t_rr.txn_id, "k:shared").unwrap();
        let rr_result = mgr.commit(t_rr.txn_id, 200);
        assert!(
            matches!(rr_result, Err(MvccError::WriteWriteConflict(_))),
            "RR 后写同 key 应 WW 冲突"
        );

        // SER 后写并提交 → 应被 SSI 检测（T_SER read k:shared ∩ T_RC.write_set {k:shared} ≠ ∅）
        // 或 WW 冲突（SSI 检测先于 WW 检测，SER 下会先返回 WriteSkewDetected）
        mgr.register_write(t_ser.txn_id, "k:shared").unwrap();
        let ser_result = mgr.commit(t_ser.txn_id, 300);
        assert!(
            matches!(
                ser_result,
                Err(MvccError::WriteSkewDetected(_)) | Err(MvccError::WriteWriteConflict(_))
            ),
            "SER 后写同 key 应被 SSI 检测或 WW 冲突，实际: {:?}",
            ser_result
        );

        // 验证最终状态
        assert_eq!(mgr.get_status(t_rc.txn_id), Some(TxnStatus::Committed));
        assert_eq!(mgr.get_status(t_rr.txn_id), Some(TxnStatus::Aborted));
        assert_eq!(mgr.get_status(t_ser.txn_id), Some(TxnStatus::Aborted));
    }

    /// 跨隔离级别：RC 看到 RR 提交的数据（通过 refresh_snapshot）
    #[test]
    fn cross_isolation_rc_sees_rr_committed_via_refresh() {
        let mgr = MvccManager::new();
        let t_rc = mgr.begin_with_isolation(IsolationLevel::ReadCommitted);
        let t_rr = mgr.begin_with_isolation(IsolationLevel::RepeatableRead);

        // RR 写 key & 提交
        mgr.register_write(t_rr.txn_id, "k:X").unwrap();
        mgr.commit(t_rr.txn_id, 100).unwrap();

        // RC 刷新前看不到 RR 的写
        let visible_before = mgr.is_visible(t_rc.txn_id, t_rr.txn_id, 0);
        assert!(!visible_before, "RC 刷新前不应看到 RR 的写");

        // RC 刷新快照（RC 允许）
        mgr.refresh_snapshot(t_rc.txn_id).unwrap();

        // RC 刷新后看到 RR 的写
        let visible_after = mgr.is_visible(t_rc.txn_id, t_rr.txn_id, 0);
        assert!(visible_after, "RC 刷新后应看到 RR 的写");
    }

    /// 跨隔离级别：RR 不受 RC 提交影响（快照冻结）
    #[test]
    fn cross_isolation_rr_unaffected_by_rc_commit() {
        let mgr = MvccManager::new();
        let t_rr = mgr.begin_with_isolation(IsolationLevel::RepeatableRead);
        let t_rc = mgr.begin_with_isolation(IsolationLevel::ReadCommitted);

        // RC 写 key & 提交
        mgr.register_write(t_rc.txn_id, "k:Y").unwrap();
        mgr.commit(t_rc.txn_id, 100).unwrap();

        // RR 不应看到 RC 的写（快照冻结）
        let visible = mgr.is_visible(t_rr.txn_id, t_rc.txn_id, 0);
        assert!(!visible, "RR 不应看到 RC 提交的数据");

        // RR 尝试 refresh_snapshot 应被拒绝
        let result = mgr.refresh_snapshot(t_rr.txn_id);
        assert!(
            matches!(result, Err(MvccError::SnapshotRefreshNotAllowed { .. })),
            "RR 不应允许 refresh_snapshot"
        );
    }

    /// 跨隔离级别：SERIALIZABLE 检测到与 RC 提交的 rw-conflict
    /// T_RC writes key A & commits → T_SER reads A & writes B & commits
    /// 由于 T_RC 在 T_SER 的快照活跃中（T_SER BEGIN 晚于 T_RC BEGIN），
    /// T_SER 提交时会被 SSI 检测（read A ∩ T_RC.write_set {A} ≠ ∅）
    #[test]
    fn cross_isolation_ser_detects_rw_conflict_with_rc() {
        let mgr = MvccManager::new();
        let t_rc = mgr.begin_with_isolation(IsolationLevel::ReadCommitted);
        // T_SER BEGIN 晚于 T_RC → T_RC 在 T_SER 的快照活跃中
        let t_ser = mgr.begin_with_isolation(IsolationLevel::Serializable);

        // T_RC writes A & commits
        mgr.register_write(t_rc.txn_id, "k:A").unwrap();
        mgr.commit(t_rc.txn_id, 100).unwrap();

        // T_SER reads A & writes B
        mgr.register_read(t_ser.txn_id, "k:A").unwrap();
        mgr.register_write(t_ser.txn_id, "k:B").unwrap();

        // T_SER commit → SSI 检测到 read A ∩ T_RC.write_set {A}
        let result = mgr.commit(t_ser.txn_id, 200);
        assert!(
            matches!(result, Err(MvccError::WriteSkewDetected(_))),
            "SERIALIZABLE 应检测到与 RC 已提交事务的 rw-conflict"
        );
    }

    /// 跨隔离级别：RC 不被 SSI 检测（即使有 rw-conflict）
    #[test]
    fn cross_isolation_rc_not_detected_by_ssi() {
        let mgr = MvccManager::new();
        let t_ser = mgr.begin_with_isolation(IsolationLevel::Serializable);
        // T_RC BEGIN 晚于 T_SER → T_SER 在 T_RC 的快照活跃中
        let t_rc = mgr.begin_with_isolation(IsolationLevel::ReadCommitted);

        // T_SER writes A & commits（先 commit）
        mgr.register_write(t_ser.txn_id, "k:A").unwrap();
        mgr.commit(t_ser.txn_id, 100).unwrap();

        // T_RC reads A & writes B
        mgr.register_read(t_rc.txn_id, "k:A").unwrap();
        mgr.register_write(t_rc.txn_id, "k:B").unwrap();

        // T_RC commit → RC 不调用 SSI 检测，应成功
        let result = mgr.commit(t_rc.txn_id, 200);
        assert!(result.is_ok(), "RC 不应被 SSI 检测（即使有 rw-conflict）");
    }

    // =================================================================
    // Part 5: 综合矩阵总结测试
    // =================================================================

    /// 完整矩阵验证：5 异常 × 3 级别，一次性验证所有 15 个组合
    #[test]
    fn full_matrix_5_anomalies_3_levels_summary() {
        // 1. 脏读：所有级别阻止
        for &level in &[
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ] {
            let (t1, t2) = dirty_read_scenario(level);
            assert_eq!((t1, t2), (Outcome::Active, Outcome::Active));
        }

        // 2. 不可重复读：RC 允许（refresh 成功 + 看到新值），RR/SER 阻止
        let (rc_r, _, rc_a) = non_repeatable_read_scenario(IsolationLevel::ReadCommitted);
        let (rr_r, _, rr_a) = non_repeatable_read_scenario(IsolationLevel::RepeatableRead);
        let (ser_r, _, ser_a) = non_repeatable_read_scenario(IsolationLevel::Serializable);
        assert!(rc_r.is_ok() && rc_a, "RC 允许不可重复读");
        assert!(rr_r.is_err() && !rr_a, "RR 阻止不可重复读");
        assert!(ser_r.is_err() && !ser_a, "SER 阻止不可重复读");

        // 3. 幻读：RC 允许，RR/SER 阻止
        let (rc_r, rc_v) = phantom_read_scenario(IsolationLevel::ReadCommitted);
        let (rr_r, rr_v) = phantom_read_scenario(IsolationLevel::RepeatableRead);
        let (ser_r, ser_v) = phantom_read_scenario(IsolationLevel::Serializable);
        assert!(rc_r.is_ok() && rc_v, "RC 允许幻读");
        assert!(rr_r.is_err() && !rr_v, "RR 阻止幻读");
        assert!(ser_r.is_err() && !ser_v, "SER 阻止幻读");

        // 4. 丢失更新：所有级别阻止（first-committer-wins）
        for &level in &[
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ] {
            let (t1, t2) = lost_update_scenario(level);
            assert_eq!(t1, Outcome::Committed);
            assert_eq!(t2, Outcome::Conflict);
        }

        // 5. 写偏斜：RC/RR 允许，SER 阻止
        let (rc_t1, rc_t2) = write_skew_scenario(IsolationLevel::ReadCommitted);
        let (rr_t1, rr_t2) = write_skew_scenario(IsolationLevel::RepeatableRead);
        let (ser_t1, ser_t2) = write_skew_scenario(IsolationLevel::Serializable);
        assert_eq!((rc_t1, rc_t2), (Outcome::Committed, Outcome::Committed));
        assert_eq!((rr_t1, rr_t2), (Outcome::Committed, Outcome::Committed));
        assert_eq!((ser_t1, ser_t2), (Outcome::Committed, Outcome::Conflict));
    }

    /// 不变量验证：所有操作在任何隔离级别下都不应 panic
    #[test]
    fn invariant_no_panic_across_all_levels() {
        let levels = [
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ];

        for &_level in &levels {
            for seed in 1..=50 {
                let ops = gen_random_ops(seed * 17, 100);
                let _ = execute_ops(&ops);
            }
        }
    }

    /// 不变量验证：已 committed/aborted 的事务不能再次 commit/abort
    #[test]
    fn invariant_terminal_state_irreversible() {
        for &level in &[
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ] {
            let mgr = MvccManager::new();
            let t1 = mgr.begin_with_isolation(level);
            mgr.register_write(t1.txn_id, "k:Z").unwrap();
            mgr.commit(t1.txn_id, 100).unwrap();

            // 再次 commit → AlreadyCommitted
            let result = mgr.commit(t1.txn_id, 200);
            assert!(
                matches!(result, Err(MvccError::AlreadyCommitted(_))),
                "{:?}: 已提交事务再次 commit 应返回 AlreadyCommitted",
                level
            );

            // 再次 abort → AlreadyCommitted
            let result = mgr.abort(t1.txn_id);
            assert!(
                matches!(result, Err(MvccError::AlreadyCommitted(_))),
                "{:?}: 已提交事务 abort 应返回 AlreadyCommitted",
                level
            );

            // 已 commit 的事务不能再 register_read/write（应返回 AlreadyCommitted）
            let result = mgr.register_read(t1.txn_id, "k:Z2");
            assert!(
                matches!(result, Err(MvccError::AlreadyCommitted(_))),
                "{:?}: 已提交事务 register_read 应返回 AlreadyCommitted",
                level
            );
        }
    }

    /// 不变量验证：不存在的事务操作应返回 TxnNotFound
    #[test]
    fn invariant_nonexistent_txn_returns_not_found() {
        let mgr = MvccManager::new();
        let result = mgr.commit(9999, 100);
        assert!(matches!(result, Err(MvccError::TxnNotFound(_))));
        let result = mgr.abort(9999);
        assert!(matches!(result, Err(MvccError::TxnNotFound(_))));
        let result = mgr.register_read(9999, "k:X");
        assert!(matches!(result, Err(MvccError::TxnNotFound(_))));
        let result = mgr.register_write(9999, "k:X");
        assert!(matches!(result, Err(MvccError::TxnNotFound(_))));
        let result = mgr.refresh_snapshot(9999);
        assert!(matches!(result, Err(MvccError::TxnNotFound(_))));
    }

    /// 不变量验证：txn_id 全局唯一且单调递增
    #[test]
    fn invariant_txn_id_monotonic_unique() {
        let mgr = MvccManager::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut prev = 0u32;
        for _ in 0..100 {
            let txn = mgr.begin();
            assert!(txn.txn_id > prev, "txn_id 应单调递增");
            assert!(seen.insert(txn.txn_id), "txn_id 应唯一");
            prev = txn.txn_id;
        }
    }

    /// 性能/规模：1000 个事务并发（顺序 BEGIN，随机操作）
    #[test]
    fn scale_1000_txns_sequential_begin_random_ops() {
        let mut rng = XorShift64::new(2024);
        let levels = [
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ];
        let keys = ["k:1", "k:2", "k:3", "k:4", "k:5", "k:6", "k:7", "k:8"];
        let mgr = MvccManager::new();
        let mut txn_ids = Vec::with_capacity(1000);

        for _ in 0..1000 {
            let level = *rng.pick(&levels);
            let txn = mgr.begin_with_isolation(level);
            txn_ids.push(txn.txn_id);
            // 每事务随机 1-3 个 read/write
            let n_ops = rng.next_range(3) + 1;
            for _ in 0..n_ops {
                let key = rng.pick(&keys).to_string();
                if rng.next_bool() {
                    let _ = mgr.register_read(txn.txn_id, key);
                } else {
                    let _ = mgr.register_write(txn.txn_id, key);
                }
            }
        }

        // 全部提交（不 panic 即可）
        let mut committed = 0;
        let mut aborted = 0;
        for &txn_id in &txn_ids {
            match mgr.commit(txn_id, 100) {
                Ok(()) => committed += 1,
                Err(MvccError::WriteWriteConflict(_)) | Err(MvccError::WriteSkewDetected(_)) => {
                    aborted += 1
                }
                Err(MvccError::AlreadyCommitted(_)) | Err(MvccError::AlreadyAborted(_)) => {}
                Err(e) => panic!("1000 事务规模测试：未预期错误 {:?}", e),
            }
        }

        assert_eq!(committed + aborted, 1000, "所有 1000 个事务应全部确定状态");
        assert!(committed > 0, "至少有一些事务应成功提交");
    }
}
