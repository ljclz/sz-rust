//! SzRSQL CDC 端到端集成测试 — 对应 `SzRSQL实施进度.md` Phase 2.5.9。
//!
//! 验证流程：
//! 1. INSERT 100000 行 → 验证 100000 个 CDC Insert 事件顺序正确
//! 2. UPDATE 50000 行 → 验证 50000 个 CDC Update 事件
//! 3. DELETE 20000 行 → 验证 20000 个 CDC Delete 事件
//!
//! # 设计要点
//!
//! 1. **不依赖真实存储引擎**：直接构造 WalRecord 列表，通过 CdcEngine 分发
//! 2. **使用 CollectingObserver**：收集所有 ChangeEvent，验证数量和顺序
//! 3. **LSN 单调递增**：验证 CDC 事件按 LSN 顺序到达
//! 4. **op 类型严格匹配**：每个阶段的 CDC 事件 op 必须与 DML 操作一致
//! 5. **多事务场景**：每个事务包含多条记录 + Commit 记录
//! 6. **大容量验证**：总计 170000 个 DML 事件 + 3 个 Commit 事件

use crate::{CdcEngine, CdcEventOp, CdcObserver, CdcObserverManager, ChangeEvent};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;
use szrsql_tx::wal::{WalOpType, WalRecord};

/// 分类收集型观察者 — 按 op 分类统计 + 保留所有事件
///
/// 与 `CollectingObserver` 的区别：分类收集 op 计数，便于按阶段验证
struct CategorizingObserver {
    events: Mutex<Vec<ChangeEvent>>,
    insert_count: std::sync::atomic::AtomicU64,
    update_count: std::sync::atomic::AtomicU64,
    delete_count: std::sync::atomic::AtomicU64,
    commit_count: std::sync::atomic::AtomicU64,
    abort_count: std::sync::atomic::AtomicU64,
}

impl CategorizingObserver {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            insert_count: std::sync::atomic::AtomicU64::new(0),
            update_count: std::sync::atomic::AtomicU64::new(0),
            delete_count: std::sync::atomic::AtomicU64::new(0),
            commit_count: std::sync::atomic::AtomicU64::new(0),
            abort_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn events(&self) -> Vec<ChangeEvent> {
        self.events.lock().clone()
    }

    fn insert_count(&self) -> u64 {
        self.insert_count.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn update_count(&self) -> u64 {
        self.update_count.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn delete_count(&self) -> u64 {
        self.delete_count.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn commit_count(&self) -> u64 {
        self.commit_count.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn abort_count(&self) -> u64 {
        self.abort_count.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn total(&self) -> u64 {
        self.insert_count()
            + self.update_count()
            + self.delete_count()
            + self.commit_count()
            + self.abort_count()
    }
}

impl CdcObserver for CategorizingObserver {
    fn on_event(&self, event: ChangeEvent) {
        match event.op {
            CdcEventOp::Insert => {
                self.insert_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Update => {
                self.update_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Delete => {
                self.delete_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Commit => {
                self.commit_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Abort => {
                self.abort_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        self.events.lock().push(event);
    }
}

/// 构造 INSERT WalRecord（含 Commit）
fn make_insert_records(tx_id: u32, start_lsn: u64, count: u64, table_id: u32) -> Vec<WalRecord> {
    let mut records = Vec::with_capacity(count as usize + 1);
    for i in 0..count {
        let lsn = start_lsn + i;
        let row_data = format!("row_{tx_id}_{i}").into_bytes();
        records.push(WalRecord::new(
            lsn,
            tx_id,
            WalOpType::Insert,
            table_id,
            row_data,
        ));
    }
    // 事务 Commit 记录
    records.push(WalRecord::new(
        start_lsn + count,
        tx_id,
        WalOpType::Commit,
        0,
        Vec::new(),
    ));
    records
}

/// 构造 UPDATE WalRecord（含 Commit）
fn make_update_records(tx_id: u32, start_lsn: u64, count: u64, table_id: u32) -> Vec<WalRecord> {
    let mut records = Vec::with_capacity(count as usize + 1);
    for i in 0..count {
        let lsn = start_lsn + i;
        let row_data = format!("updated_{tx_id}_{i}").into_bytes();
        records.push(WalRecord::new(
            lsn,
            tx_id,
            WalOpType::Update,
            table_id,
            row_data,
        ));
    }
    records.push(WalRecord::new(
        start_lsn + count,
        tx_id,
        WalOpType::Commit,
        0,
        Vec::new(),
    ));
    records
}

/// 构造 DELETE WalRecord（含 Commit）
fn make_delete_records(tx_id: u32, start_lsn: u64, count: u64, table_id: u32) -> Vec<WalRecord> {
    let mut records = Vec::with_capacity(count as usize + 1);
    for i in 0..count {
        let lsn = start_lsn + i;
        let row_data = format!("deleted_{tx_id}_{i}").into_bytes();
        records.push(WalRecord::new(
            lsn,
            tx_id,
            WalOpType::Delete,
            table_id,
            row_data,
        ));
    }
    records.push(WalRecord::new(
        start_lsn + count,
        tx_id,
        WalOpType::Commit,
        0,
        Vec::new(),
    ));
    records
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_tx::wal::WalObserver;

    // =================================================================
    // Part 1: 基础流程验证（小规模）
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_small_insert_flow() {
        // 小规模 INSERT 流程：100 行 + 1 Commit
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        let records = make_insert_records(1, 100, 100, 42);
        engine.on_commit(1, records);

        assert_eq!(observer.insert_count(), 100);
        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.update_count(), 0);
        assert_eq!(observer.delete_count(), 0);
        assert_eq!(observer.abort_count(), 0);
        assert_eq!(observer.total(), 101);
    }

    #[test]
    fn phase_2_5_9_e2e_small_update_flow() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        let records = make_update_records(2, 200, 50, 42);
        engine.on_commit(2, records);

        assert_eq!(observer.update_count(), 50);
        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.insert_count(), 0);
        assert_eq!(observer.delete_count(), 0);
    }

    #[test]
    fn phase_2_5_9_e2e_small_delete_flow() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        let records = make_delete_records(3, 300, 20, 42);
        engine.on_commit(3, records);

        assert_eq!(observer.delete_count(), 20);
        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.insert_count(), 0);
        assert_eq!(observer.update_count(), 0);
    }

    // =================================================================
    // Part 2: 完整端到端流程（INSERT 100K + UPDATE 50K + DELETE 20K）
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_full_flow_insert_update_delete() {
        // 完整流程：
        // 1. INSERT 100000 行 → 100000 Insert 事件 + 1 Commit
        // 2. UPDATE 50000 行 → 50000 Update 事件 + 1 Commit
        // 3. DELETE 20000 行 → 20000 Delete 事件 + 1 Commit
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // === 阶段 1：INSERT 100000 行 ===
        let insert_records = make_insert_records(10, 1_000_000, 100_000, 42);
        engine.on_commit(10, insert_records);

        assert_eq!(
            observer.insert_count(),
            100_000,
            "阶段 1：应有 100000 个 Insert 事件"
        );
        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.update_count(), 0);
        assert_eq!(observer.delete_count(), 0);

        // === 阶段 2：UPDATE 50000 行 ===
        let update_records = make_update_records(11, 2_000_000, 50_000, 42);
        engine.on_commit(11, update_records);

        assert_eq!(
            observer.insert_count(),
            100_000,
            "阶段 2：Insert 事件数不变"
        );
        assert_eq!(
            observer.update_count(),
            50_000,
            "阶段 2：应有 50000 个 Update 事件"
        );
        assert_eq!(observer.delete_count(), 0);
        assert_eq!(observer.commit_count(), 2);

        // === 阶段 3：DELETE 20000 行 ===
        let delete_records = make_delete_records(12, 3_000_000, 20_000, 42);
        engine.on_commit(12, delete_records);

        assert_eq!(observer.insert_count(), 100_000);
        assert_eq!(observer.update_count(), 50_000);
        assert_eq!(
            observer.delete_count(),
            20_000,
            "阶段 3：应有 20000 个 Delete 事件"
        );
        assert_eq!(observer.commit_count(), 3);
        assert_eq!(observer.abort_count(), 0);

        // 总事件数：100000 + 50000 + 20000 + 3 = 170003
        assert_eq!(observer.total(), 170_003);
    }

    // =================================================================
    // Part 3: LSN 顺序验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_lsn_monotonic_within_phase() {
        // 同一阶段内 LSN 严格单调递增
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        let records = make_insert_records(1, 100, 1000, 42);
        engine.on_commit(1, records);

        let events = observer.events();
        // 过滤 Insert 事件（排除 Commit）
        let insert_events: Vec<&ChangeEvent> = events
            .iter()
            .filter(|e| e.op == CdcEventOp::Insert)
            .collect();
        assert_eq!(insert_events.len(), 1000);

        // 验证 LSN 严格单调递增
        for i in 1..insert_events.len() {
            assert!(
                insert_events[i].lsn > insert_events[i - 1].lsn,
                "LSN 不是单调递增：位置 {} lsn={} 前一 lsn={}",
                i,
                insert_events[i].lsn,
                insert_events[i - 1].lsn
            );
        }
    }

    #[test]
    fn phase_2_5_9_e2e_lsn_monotonic_across_phases() {
        // 跨阶段 LSN 单调递增
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 阶段 1：LSN 100~109（10 个 Insert + 1 Commit at LSN 110）
        engine.on_commit(1, make_insert_records(1, 100, 10, 42));
        // 阶段 2：LSN 200~209（10 个 Update + 1 Commit at LSN 210）
        engine.on_commit(2, make_update_records(2, 200, 10, 42));
        // 阶段 3：LSN 300~309（10 个 Delete + 1 Commit at LSN 310）
        engine.on_commit(3, make_delete_records(3, 300, 10, 42));

        let events = observer.events();
        assert_eq!(events.len(), 33);

        // 所有事件 LSN 单调递增
        for i in 1..events.len() {
            assert!(
                events[i].lsn > events[i - 1].lsn,
                "跨阶段 LSN 不单调：位置 {} lsn={} 前一 lsn={}",
                i,
                events[i].lsn,
                events[i - 1].lsn
            );
        }
    }

    // =================================================================
    // Part 4: op 类型严格匹配
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_op_types_match_dml_operations() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 阶段 1：INSERT → 所有非 Commit 事件必须是 Insert
        engine.on_commit(1, make_insert_records(1, 100, 100, 42));
        let events_after_phase1 = observer.events();
        for event in &events_after_phase1 {
            match event.op {
                CdcEventOp::Insert | CdcEventOp::Commit => {}
                other => panic!("阶段 1 出现非预期 op: {:?}", other),
            }
        }

        // 阶段 2：UPDATE → 新增事件必须是 Update 或 Commit
        let count_after_phase1 = events_after_phase1.len();
        engine.on_commit(2, make_update_records(2, 200, 50, 42));
        let events_after_phase2 = observer.events();
        for event in &events_after_phase2[count_after_phase1..] {
            match event.op {
                CdcEventOp::Update | CdcEventOp::Commit => {}
                other => panic!("阶段 2 出现非预期 op: {:?}", other),
            }
        }

        // 阶段 3：DELETE → 新增事件必须是 Delete 或 Commit
        let count_after_phase2 = events_after_phase2.len();
        engine.on_commit(3, make_delete_records(3, 300, 20, 42));
        let events_after_phase3 = observer.events();
        for event in &events_after_phase3[count_after_phase2..] {
            match event.op {
                CdcEventOp::Delete | CdcEventOp::Commit => {}
                other => panic!("阶段 3 出现非预期 op: {:?}", other),
            }
        }
    }

    // =================================================================
    // Part 5: tx_id 隔离验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_tx_id_isolation() {
        // 不同事务的 tx_id 严格隔离，不串台
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 事务 100：100 Insert
        engine.on_commit(100, make_insert_records(100, 1000, 100, 42));
        // 事务 200：50 Update
        engine.on_commit(200, make_update_records(200, 2000, 50, 42));
        // 事务 300：20 Delete
        engine.on_commit(300, make_delete_records(300, 3000, 20, 42));

        let events = observer.events();

        // 阶段 1 所有事件 tx_id == 100
        for event in &events[..101] {
            assert_eq!(event.tx_id, 100, "阶段 1 事件 tx_id 应为 100");
        }
        // 阶段 2 所有事件 tx_id == 200
        for event in &events[101..152] {
            assert_eq!(event.tx_id, 200, "阶段 2 事件 tx_id 应为 200");
        }
        // 阶段 3 所有事件 tx_id == 300
        for event in &events[152..] {
            assert_eq!(event.tx_id, 300, "阶段 3 事件 tx_id 应为 300");
        }
    }

    // =================================================================
    // Part 6: table_id 透传验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_table_id_passthrough() {
        // DML 事件 table_id 与 WalRecord 一致，Commit 事件 table_id 为 None
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 表 1000 的 10 个 Insert
        engine.on_commit(1, make_insert_records(1, 100, 10, 1000));
        // 表 2000 的 5 个 Update
        engine.on_commit(2, make_update_records(2, 200, 5, 2000));
        // 表 3000 的 3 个 Delete
        engine.on_commit(3, make_delete_records(3, 300, 3, 3000));

        let events = observer.events();
        // Insert 事件 table_id = Some(1000)
        for event in events.iter().filter(|e| e.op == CdcEventOp::Insert) {
            assert_eq!(event.table_id, Some(1000));
        }
        // Update 事件 table_id = Some(2000)
        for event in events.iter().filter(|e| e.op == CdcEventOp::Update) {
            assert_eq!(event.table_id, Some(2000));
        }
        // Delete 事件 table_id = Some(3000)
        for event in events.iter().filter(|e| e.op == CdcEventOp::Delete) {
            assert_eq!(event.table_id, Some(3000));
        }
        // Commit 事件 table_id = None
        for event in events.iter().filter(|e| e.op == CdcEventOp::Commit) {
            assert_eq!(event.table_id, None);
        }
    }

    // =================================================================
    // Part 7: 多 observer 并发分发验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_multiple_observers_all_receive_events() {
        // 注册 3 个 observer，所有 observer 都应收到全部事件
        let observer1 = Arc::new(CategorizingObserver::new());
        let observer2 = Arc::new(CategorizingObserver::new());
        let observer3 = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer1.clone());
        mgr.register(observer2.clone());
        mgr.register(observer3.clone());
        let engine = CdcEngine::new(mgr.clone());

        engine.on_commit(1, make_insert_records(1, 100, 100, 42));
        engine.on_commit(2, make_update_records(2, 200, 50, 42));
        engine.on_commit(3, make_delete_records(3, 300, 20, 42));

        // 3 个 observer 都收到相同数量的事件
        for (i, obs) in [observer1.clone(), observer2.clone(), observer3.clone()]
            .iter()
            .enumerate()
        {
            assert_eq!(obs.insert_count(), 100, "observer{} insert_count", i + 1);
            assert_eq!(obs.update_count(), 50, "observer{} update_count", i + 1);
            assert_eq!(obs.delete_count(), 20, "observer{} delete_count", i + 1);
            assert_eq!(obs.commit_count(), 3, "observer{} commit_count", i + 1);
            assert_eq!(obs.total(), 173, "observer{} total", i + 1);
        }

        // 引擎统计：3 observer × 173 events = 519 次分发
        assert_eq!(engine.total_dispatched(), 519);
        assert_eq!(engine.observer_count(), 3);
    }

    // =================================================================
    // Part 8: 事务回滚（Abort）事件
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_rollback_generates_abort_event() {
        // on_rollback 应生成 1 个 Abort 事件
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        engine.on_rollback(999);

        assert_eq!(observer.abort_count(), 1);
        assert_eq!(observer.insert_count(), 0);
        assert_eq!(observer.update_count(), 0);
        assert_eq!(observer.delete_count(), 0);
        assert_eq!(observer.commit_count(), 0);
        assert_eq!(observer.total(), 1);

        let events = observer.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].op, CdcEventOp::Abort);
        assert_eq!(events[0].tx_id, 999);
        assert_eq!(events[0].table_id, None);
    }

    // =================================================================
    // Part 9: 混合事务（Insert + Update + Delete + Commit）
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_mixed_transaction_single_commit() {
        // 单个事务包含 Insert + Update + Delete + Commit
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        let mut records = Vec::new();
        // 5 Insert (LSN 100-104)
        for i in 0..5 {
            records.push(WalRecord::new(
                100 + i,
                1,
                WalOpType::Insert,
                42,
                format!("ins_{i}").into_bytes(),
            ));
        }
        // 3 Update (LSN 105-107)
        for i in 0..3 {
            records.push(WalRecord::new(
                105 + i,
                1,
                WalOpType::Update,
                42,
                format!("upd_{i}").into_bytes(),
            ));
        }
        // 2 Delete (LSN 108-109)
        for i in 0..2 {
            records.push(WalRecord::new(
                108 + i,
                1,
                WalOpType::Delete,
                42,
                format!("del_{i}").into_bytes(),
            ));
        }
        // Commit (LSN 110)
        records.push(WalRecord::new(110, 1, WalOpType::Commit, 0, Vec::new()));

        engine.on_commit(1, records);

        assert_eq!(observer.insert_count(), 5);
        assert_eq!(observer.update_count(), 3);
        assert_eq!(observer.delete_count(), 2);
        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.total(), 11);

        // 验证 LSN 单调递增
        let events = observer.events();
        for i in 1..events.len() {
            assert!(events[i].lsn > events[i - 1].lsn);
        }

        // 所有事件 tx_id = 1
        for event in &events {
            assert_eq!(event.tx_id, 1);
        }
    }

    // =================================================================
    // Part 10: 过滤验证（FullPageImage / Checkpoint 不产生 CDC 事件）
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_filters_full_page_image_and_checkpoint() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        let records = vec![
            // FullPageImage - 应被过滤
            WalRecord::new(100, 1, WalOpType::FullPageImage, 42, vec![0; 100]),
            // Insert - 应保留
            WalRecord::new(101, 1, WalOpType::Insert, 42, vec![1]),
            // Checkpoint - 应被过滤
            WalRecord::new(102, 1, WalOpType::Checkpoint, 0, Vec::new()),
            // Update - 应保留
            WalRecord::new(103, 1, WalOpType::Update, 42, vec![2]),
            // Commit - 应保留
            WalRecord::new(104, 1, WalOpType::Commit, 0, Vec::new()),
        ];

        engine.on_commit(1, records);

        // 只应有 1 Insert + 1 Update + 1 Commit = 3 事件
        assert_eq!(observer.insert_count(), 1);
        assert_eq!(observer.update_count(), 1);
        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.total(), 3);
    }

    // =================================================================
    // Part 11: 引擎统计验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_engine_stats_after_full_flow() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 阶段 1：100000 Insert + 1 Commit = 100001 records
        engine.on_commit(1, make_insert_records(1, 1_000_000, 100_000, 42));
        // 阶段 2：50000 Update + 1 Commit = 50001 records
        engine.on_commit(2, make_update_records(2, 2_000_000, 50_000, 42));
        // 阶段 3：20000 Delete + 1 Commit = 20001 records
        engine.on_commit(3, make_delete_records(3, 3_000_000, 20_000, 42));

        // total_processed：3 个事务共 170003 个 WalRecord
        assert_eq!(engine.total_processed(), 170_003);

        // total_dispatched：1 observer × 170003 events = 170003
        assert_eq!(engine.total_dispatched(), 170_003);

        // observer_count：1
        assert_eq!(engine.observer_count(), 1);

        // pending_events：同步分发，始终为 0
        assert_eq!(engine.pending_event_count(), 0);
    }

    // =================================================================
    // Part 12: 空事务验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_empty_transaction_no_events() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 空记录列表（无 Commit）
        engine.on_commit(1, Vec::new());

        assert_eq!(observer.total(), 0);
        assert_eq!(engine.total_processed(), 0);
    }

    #[test]
    fn phase_2_5_9_e2e_empty_transaction_with_only_commit() {
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 仅 1 个 Commit 记录
        let records = vec![WalRecord::new(100, 1, WalOpType::Commit, 0, Vec::new())];
        engine.on_commit(1, records);

        assert_eq!(observer.commit_count(), 1);
        assert_eq!(observer.total(), 1);
        assert_eq!(engine.total_processed(), 1);
    }

    // =================================================================
    // Part 13: 大规模 stress 验证（300K 事件）
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_stress_300k_events_mixed() {
        // Stress：3 阶段共 300000 DML + 3 Commit = 300003 事件
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 阶段 1：200000 Insert
        engine.on_commit(1, make_insert_records(1, 1_000_000, 200_000, 42));
        assert_eq!(observer.insert_count(), 200_000);

        // 阶段 2：80000 Update
        engine.on_commit(2, make_update_records(2, 2_000_000, 80_000, 42));
        assert_eq!(observer.update_count(), 80_000);

        // 阶段 3：20000 Delete
        engine.on_commit(3, make_delete_records(3, 3_000_000, 20_000, 42));
        assert_eq!(observer.delete_count(), 20_000);

        assert_eq!(observer.commit_count(), 3);
        assert_eq!(observer.total(), 300_003);
        assert_eq!(engine.total_processed(), 300_003);
        assert_eq!(engine.total_dispatched(), 300_003);
    }

    // =================================================================
    // Part 14: 跨多事务顺序消费验证
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_cross_transaction_ordering() {
        // 多个事务交错提交，验证事件按提交顺序到达
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 事务 1：5 Insert (LSN 100-104) + Commit (LSN 105)
        engine.on_commit(1, make_insert_records(1, 100, 5, 42));
        // 事务 2：3 Update (LSN 200-202) + Commit (LSN 203)
        engine.on_commit(2, make_update_records(2, 200, 3, 42));
        // 事务 3：2 Delete (LSN 300-301) + Commit (LSN 302)
        engine.on_commit(3, make_delete_records(3, 300, 2, 42));
        // 事务 4：4 Insert (LSN 400-403) + Commit (LSN 404)
        engine.on_commit(4, make_insert_records(4, 400, 4, 42));

        let events = observer.events();
        // 总事件数：5+1 + 3+1 + 2+1 + 4+1 = 18
        assert_eq!(events.len(), 18);

        // 验证提交顺序：先事务1，再事务2，再事务3，再事务4
        let tx_ids: Vec<u32> = events.iter().map(|e| e.tx_id).collect();
        let expected_tx_ids: Vec<u32> = vec![
            1, 1, 1, 1, 1, 1, // 事务1：5 Insert + 1 Commit
            2, 2, 2, 2, // 事务2：3 Update + 1 Commit
            3, 3, 3, // 事务3：2 Delete + 1 Commit
            4, 4, 4, 4, 4, // 事务4：4 Insert + 1 Commit
        ];
        assert_eq!(tx_ids, expected_tx_ids);

        // 每个 Commit 事件必须是该事务最后一个事件
        let commit_events: Vec<&ChangeEvent> = events
            .iter()
            .filter(|e| e.op == CdcEventOp::Commit)
            .collect();
        assert_eq!(commit_events.len(), 4);
        assert_eq!(commit_events[0].tx_id, 1);
        assert_eq!(commit_events[1].tx_id, 2);
        assert_eq!(commit_events[2].tx_id, 3);
        assert_eq!(commit_events[3].tx_id, 4);
    }

    // =================================================================
    // Part 15: 完整流程 + 全部 op 类型统计
    // =================================================================

    #[test]
    fn phase_2_5_9_e2e_full_flow_with_abort() {
        // 完整流程含回滚：INSERT + UPDATE + DELETE + Abort
        let observer = Arc::new(CategorizingObserver::new());
        let mgr = Arc::new(CdcObserverManager::new());
        mgr.register(observer.clone());
        let engine = CdcEngine::new(mgr.clone());

        // 提交事务 1：1000 Insert
        engine.on_commit(1, make_insert_records(1, 1000, 1000, 42));
        // 回滚事务 2
        engine.on_rollback(2);
        // 提交事务 3：500 Update
        engine.on_commit(3, make_update_records(3, 2000, 500, 42));
        // 回滚事务 4
        engine.on_rollback(4);
        // 提交事务 5：200 Delete
        engine.on_commit(5, make_delete_records(5, 3000, 200, 42));

        assert_eq!(observer.insert_count(), 1000);
        assert_eq!(observer.update_count(), 500);
        assert_eq!(observer.delete_count(), 200);
        assert_eq!(observer.commit_count(), 3);
        assert_eq!(observer.abort_count(), 2);
        assert_eq!(observer.total(), 1705);

        // total_processed 只统计 on_commit 的 records，不统计 on_rollback
        // 事务1：1001 records，事务3：501 records，事务5：201 records = 1703
        assert_eq!(engine.total_processed(), 1703);
    }
}
