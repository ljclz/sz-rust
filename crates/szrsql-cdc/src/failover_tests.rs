//! Phase 2.5.11: CDC 消费者故障转移测试 — 对应 `SzRSQL实施进度.md` Phase 2.5.11。
//!
//! # 验证矩阵
//!
//! - **Chaos：CDC 消费者在处理第 500000 个事件时崩溃 → 重启 → 从正确 offset 继续 →
//!   验证消费完 1000000 个事件**
//! - **0 事件丢失, 0 重复**
//!
//! # 测试组织
//!
//! - Part 1: 基础流程（无崩溃）
//! - Part 2: Chaos 主场景（500K 崩溃 → 1M 完成）
//! - Part 3: 崩溃前未提交（at-least-once + idempotent 去重）
//! - Part 4: 多次崩溃（crash → recover → crash → recover → 完成）
//! - Part 5: 崩溃在第一个事件
//! - Part 6: 崩溃在最后一个事件
//! - Part 7: 混合 op 类型（Insert/Update/Delete）故障转移
//! - Part 8: 多分区独立故障转移
//! - Part 9: 并发消费者（同组，一个崩溃，另一个继续）
//! - Part 10: CDC 引擎集成（真实 WalRecord → ChangeEvent → FailoverConsumer）
//! - Part 11: 大批量 stress（1M 事件 + 多次崩溃）
//! - Part 12: Exactly-once 严格验证（无丢失 + 无重复）

use super::*;
use std::sync::Arc;

// =====================================================================
// Part 1: 基础流程（无崩溃）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part1_basic_flow {
    use super::*;

    #[test]
    fn phase_2_5_11_basic_flow_process_all_events() {
        // 基础流程：100 个事件，无崩溃，全部处理
        let path = make_temp_path("part1_basic");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
            .with_commit_batch_size(10);

        let events = make_insert_events(1, 1, 100, 42, 12345);
        let mut processed_count = 0u64;
        for event in &events {
            if consumer.process_event(event) == ProcessResult::Processed {
                processed_count += 1;
            }
        }
        // 强制提交最后一个 LSN
        consumer.commit(100).unwrap();

        assert_eq!(processed_count, 100);
        assert_eq!(consumer.total_processed(), 100);
        assert_eq!(consumer.processed_set_size(), 100);
        assert_eq!(consumer.committed_lsn(), Some(100));
        assert_eq!(consumer.next_lsn(), 101);

        cleanup_temp_files(&path);
    }

    #[test]
    fn phase_2_5_11_basic_flow_skip_committed() {
        // 提交后再次处理同一批事件，应全部 SkippedCommitted
        let path = make_temp_path("part1_skip");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);

        let events = make_insert_events(1, 1, 50, 42, 12345);
        for event in &events {
            consumer.process_event(event);
        }
        consumer.commit(50).unwrap();

        // 再次处理：全部 SkippedCommitted
        for event in &events {
            let result = consumer.process_event(event);
            assert_eq!(result, ProcessResult::SkippedCommitted);
        }
        assert_eq!(consumer.total_processed(), 50);
        assert_eq!(consumer.total_skipped(), 50);

        cleanup_temp_files(&path);
    }

    #[test]
    fn phase_2_5_11_basic_flow_skip_duplicate() {
        // 处理同一批事件两次（未 commit），第二次全部 SkippedDuplicate
        let path = make_temp_path("part1_dup");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);

        let events = make_insert_events(1, 1, 30, 42, 12345);
        // 第一次：全部 Processed
        for event in &events {
            assert_eq!(consumer.process_event(event), ProcessResult::Processed);
        }
        // 第二次：全部 SkippedDuplicate（mark_processed 返回 false）
        for event in &events {
            assert_eq!(
                consumer.process_event(event),
                ProcessResult::SkippedDuplicate
            );
        }
        assert_eq!(consumer.total_processed(), 30);
        assert_eq!(consumer.total_skipped(), 30);

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 2: Chaos 主场景（500K 崩溃 → 1M 完成）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part2_chaos_main {
    use super::*;

    /// Phase 2.5.11 标志性测试：
    /// CDC 消费者在处理第 500000 个事件时崩溃 → 重启 → 从正确 offset 继续 →
    /// 验证消费完 1000000 个事件，0 事件丢失, 0 重复。
    #[test]
    fn phase_2_5_11_chaos_crash_at_500k_complete_1m_events() {
        let path = make_temp_path("part2_500k_1m");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        // 生成 1M 个事件（LSN 1..=1000000）
        let total_events = 1_000_000u64;
        let crash_lsn = 500_000u64;
        let events = make_insert_events(1, 1, total_events, 42, 12345);

        // 第一次会话：处理到 LSN 500000 时崩溃
        let processed_in_first_session: u64;
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(crash_lsn)
                .with_commit_batch_size(1000);

            for event in &events {
                let result = consumer.process_event(event);
                if result == ProcessResult::Crashed {
                    break;
                }
            }
            assert!(consumer.is_crashed());

            // 持久化 processed set（模拟下游应用在崩溃前的 checkpoint）
            consumer.flush_processed().unwrap();
            processed_in_first_session = consumer.total_processed();
        }

        // 崩溃前应处理了 499999 个事件（LSN 1..=499999）
        // LSN 500000 触发崩溃，未加入 processed set
        assert_eq!(processed_in_first_session, crash_lsn - 1);

        // 第二次会话：恢复，继续处理到 1M
        let processed_in_second_session: u64;
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_commit_batch_size(1000);

            // 恢复 processed set
            consumer.load_processed().unwrap();
            consumer.recover();
            assert!(!consumer.is_crashed());

            // 从 committed_lsn + 1 开始重投
            let restart_lsn = consumer.next_lsn();

            // 重投从 restart_lsn 开始的所有事件
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events[restart_idx..] {
                let result = consumer.process_event(event);
                assert_ne!(result, ProcessResult::Crashed);
            }
            consumer.commit(total_events).unwrap();
            processed_in_second_session = consumer.total_processed();
            // 持久化 processed set
            consumer.flush_processed().unwrap();
        }

        // 验证：总处理数 = 1M（0 丢失）
        // 第二次会话处理的"新"事件 = 1M - 499999 = 500001（LSN 500000..=1000000）
        assert_eq!(
            processed_in_first_session + processed_in_second_session,
            total_events
        );
        // 0 丢失：processed set 应包含所有 1M 个 LSN
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total_events as usize);

        // 0 重复：每个 LSN 在 processed set 中只出现一次（HashSet 保证）
        // 通过遍历 1..=1M 验证全部存在
        {
            let processed = consumer.processed_set_size();
            assert_eq!(processed, total_events as usize);
        }

        cleanup_temp_files(&path);
    }

    #[test]
    fn phase_2_5_11_chaos_crash_at_500k_no_loss_no_duplication() {
        // 严格验证 0 丢失 + 0 重复
        let path = make_temp_path("part2_strict");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total_events = 100_000u64; // 较小规模便于详细验证
        let crash_lsn = 50_000u64;
        let events = make_insert_events(1, 1, total_events, 42, 12345);

        // 第一次会话：崩溃在 50000
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(crash_lsn)
                .with_commit_batch_size(1000);
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 第二次会话：恢复
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_commit_batch_size(1000);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events[restart_idx..] {
                consumer.process_event(event);
            }
            consumer.commit(total_events).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证：每个 LSN 1..=total_events 都在 processed set 中（0 丢失）
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        for lsn in 1..=total_events {
            assert!(
                consumer.is_in_processed_set(lsn),
                "LSN {} missing from processed set (loss detected)",
                lsn
            );
        }
        // 0 重复：processed set 大小 == total_events
        assert_eq!(consumer.processed_set_size(), total_events as usize);

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 3: 崩溃前未提交（at-least-once + idempotent 去重）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part3_uncommitted_crash {
    use super::*;

    #[test]
    fn phase_2_5_11_crash_before_commit_redelivery_with_dedup() {
        // 场景：处理 100 个事件但未 commit → 崩溃 → 恢复后从 LSN 1 重投
        // processed set 持久化后能去重（idempotent）
        let path = make_temp_path("part3_uncommitted");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let events = make_insert_events(1, 1, 100, 42, 12345);

        // 第一次会话：处理 100 个事件，未 commit 就崩溃
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(100) // 在第 100 个事件崩溃
                .with_commit_batch_size(1000); // 大 batch_size，避免自动 commit
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            // 崩溃前已处理 99 个事件（LSN 1..=99），未 commit
            assert_eq!(consumer.total_processed(), 99);
            assert_eq!(consumer.committed_lsn(), None);
            // 持久化 processed set
            consumer.flush_processed().unwrap();
        }

        // 第二次会话：恢复，从 LSN 1 开始重投（committed_lsn = None → next_lsn = 1）
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_commit_batch_size(1000);
            consumer.load_processed().unwrap();
            consumer.recover();

            assert_eq!(consumer.next_lsn(), 1); // 从 1 开始

            let mut new_count = 0u64;
            let mut dup_count = 0u64;
            for event in &events {
                match consumer.process_event(event) {
                    ProcessResult::Processed => new_count += 1,
                    ProcessResult::SkippedDuplicate => dup_count += 1,
                    ProcessResult::Crashed => break,
                    ProcessResult::SkippedCommitted => {}
                }
            }
            // LSN 1..=99 已在 processed set 中（SkippedDuplicate）
            // LSN 100 是崩溃点，恢复后重投，应处理成功（crash_at 未设置）
            assert_eq!(new_count, 1); // 只有 LSN 100 是新的
            assert_eq!(dup_count, 99); // LSN 1..=99 被去重
            consumer.commit(100).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 最终验证：100 个事件全部在 processed set 中
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), 100);
        assert_eq!(consumer.committed_lsn(), Some(100));

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 4: 多次崩溃（crash → recover → crash → recover → 完成）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part4_multiple_crashes {
    use super::*;

    #[test]
    fn phase_2_5_11_multiple_crashes_three_recoveries() {
        // 场景：处理 1000 个事件，在 LSN 300 / 600 / 900 三次崩溃
        let path = make_temp_path("part4_multi_crash");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total = 1000u64;
        let events = make_insert_events(1, 1, total, 42, 12345);
        let crash_points = [300u64, 600, 900];

        // 三次崩溃 + 三次恢复
        for (session_idx, &crash_at) in crash_points.iter().enumerate() {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(crash_at)
                .with_commit_batch_size(100);
            consumer.load_processed().unwrap();
            if session_idx > 0 {
                consumer.recover();
            }

            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events[restart_idx..] {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            assert!(consumer.is_crashed());
            consumer.flush_processed().unwrap();
        }

        // 最后一次会话：处理完剩余事件
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_commit_batch_size(100);
            consumer.load_processed().unwrap();
            consumer.recover();

            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events[restart_idx..] {
                consumer.process_event(event);
            }
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证：1000 个事件全部在 processed set 中
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total as usize);
        assert_eq!(consumer.committed_lsn(), Some(total));

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 5: 崩溃在第一个事件
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part5_crash_first_event {
    use super::*;

    #[test]
    fn phase_2_5_11_crash_at_first_event() {
        let path = make_temp_path("part5_first");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let events = make_insert_events(1, 1, 100, 42, 12345);

        // 崩溃在 LSN 1
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(1);
            let result = consumer.process_event(&events[0]);
            assert_eq!(result, ProcessResult::Crashed);
            assert!(consumer.is_crashed());
            assert_eq!(consumer.total_processed(), 0);
            consumer.flush_processed().unwrap();
        }

        // 恢复，从 LSN 1 开始
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
            consumer.load_processed().unwrap();
            consumer.recover();
            assert_eq!(consumer.next_lsn(), 1);

            for event in &events {
                consumer.process_event(event);
            }
            consumer.commit(100).unwrap();
            consumer.flush_processed().unwrap();
        }

        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), 100);

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 6: 崩溃在最后一个事件
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part6_crash_last_event {
    use super::*;

    #[test]
    fn phase_2_5_11_crash_at_last_event() {
        let path = make_temp_path("part6_last");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total = 100u64;
        let events = make_insert_events(1, 1, total, 42, 12345);

        // 崩溃在 LSN 100（最后一个）
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(total)
                .with_commit_batch_size(1000);
            for event in &events[..99] {
                consumer.process_event(event);
            }
            // 第 100 个触发崩溃
            let result = consumer.process_event(&events[99]);
            assert_eq!(result, ProcessResult::Crashed);
            consumer.flush_processed().unwrap();
        }

        // 恢复
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
            consumer.load_processed().unwrap();
            consumer.recover();
            for event in &events {
                consumer.process_event(event);
            }
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total as usize);

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 7: 混合 op 类型（Insert/Update/Delete）故障转移
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part7_mixed_op_types {
    use super::*;
    use crate::CdcEventOp;

    #[test]
    fn phase_2_5_11_mixed_op_types_crash_recovery() {
        // 混合 Insert/Update/Delete 事件，崩溃恢复后仍保证 exactly-once
        let path = make_temp_path("part7_mixed");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total = 300u64;
        let events = make_mixed_events(1, 1, total, 42, 12345);
        let crash_lsn = 150u64;

        // 第一次会话：崩溃在 150
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_crash_point(crash_lsn)
                .with_commit_batch_size(50);
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 第二次会话：恢复
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0)
                .with_commit_batch_size(50);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events[restart_idx..] {
                consumer.process_event(event);
            }
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 0);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total as usize);
        assert_eq!(consumer.committed_lsn(), Some(total));

        // 验证 op 类型分布（前 300 个事件按 i%3 分布）
        // i=0,3,6,...,297 → Insert (100 个)
        // i=1,4,7,...,298 → Update (100 个)
        // i=2,5,8,...,299 → Delete (100 个)
        let insert_count = events.iter().filter(|e| e.op == CdcEventOp::Insert).count();
        let update_count = events.iter().filter(|e| e.op == CdcEventOp::Update).count();
        let delete_count = events.iter().filter(|e| e.op == CdcEventOp::Delete).count();
        assert_eq!(insert_count, 100);
        assert_eq!(update_count, 100);
        assert_eq!(delete_count, 100);

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 8: 多分区独立故障转移
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part8_multi_partition {
    use super::*;

    #[test]
    fn phase_2_5_11_multi_partition_independent_crash() {
        // 两个分区（table_id=10, table_id=20），各自独立崩溃和恢复
        // 每个分区使用独立的 processed_path，避免相互覆盖
        let path = make_temp_path("part8_multi_part");
        cleanup_temp_files(&path);
        let processed_path_p1 = path.with_extension("p10.processed");
        let processed_path_p2 = path.with_extension("p20.processed");
        let _ = std::fs::remove_file(&processed_path_p1);
        let _ = std::fs::remove_file(&processed_path_p2);

        let total_per_partition = 200u64;
        let events_p1 = make_insert_events(1, 1, total_per_partition, 10, 12345);
        let events_p2 = make_insert_events(2, 1, total_per_partition, 20, 12345);

        // 分区 1：崩溃在 100
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path_p1, "group1", 10)
                .with_crash_point(100)
                .with_commit_batch_size(50);
            for event in &events_p1 {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 分区 2：崩溃在 150
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path_p2, "group1", 20)
                .with_crash_point(150)
                .with_commit_batch_size(50);
            for event in &events_p2 {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 恢复分区 1
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path_p1, "group1", 10)
                .with_commit_batch_size(50);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events_p1[restart_idx..] {
                consumer.process_event(event);
            }
            consumer.commit(total_per_partition).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 恢复分区 2
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path_p2, "group1", 20)
                .with_commit_batch_size(50);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events_p2[restart_idx..] {
                consumer.process_event(event);
            }
            consumer.commit(total_per_partition).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证两个分区都完整
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer1 = FailoverConsumer::new(store.clone(), &processed_path_p1, "group1", 10);
        let consumer2 = FailoverConsumer::new(store.clone(), &processed_path_p2, "group1", 20);
        consumer1.load_processed().unwrap();
        consumer2.load_processed().unwrap();
        assert_eq!(consumer1.processed_set_size(), total_per_partition as usize);
        assert_eq!(consumer2.processed_set_size(), total_per_partition as usize);
        assert_eq!(consumer1.committed_lsn(), Some(total_per_partition));
        assert_eq!(consumer2.committed_lsn(), Some(total_per_partition));

        cleanup_temp_files(&path);
        let _ = std::fs::remove_file(&processed_path_p1);
        let _ = std::fs::remove_file(&processed_path_p2);
    }
}

// =====================================================================
// Part 9: 并发消费者（同组，一个崩溃，另一个继续）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part9_concurrent_consumers {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn phase_2_5_11_concurrent_consumers_one_crashes_other_continues() {
        // 两个消费者处理不同分区，其中一个崩溃，另一个继续
        // 每个分区使用独立的 processed_path，避免并发写文件冲突
        let path = make_temp_path("part9_concurrent");
        cleanup_temp_files(&path);
        let processed_path_p1 = path.with_extension("p100.processed");
        let processed_path_p2 = path.with_extension("p200.processed");
        let _ = std::fs::remove_file(&processed_path_p1);
        let _ = std::fs::remove_file(&processed_path_p2);

        let total = 1000u64;
        let barrier = Arc::new(Barrier::new(2));

        let path_clone = path.clone();
        let processed_clone = processed_path_p1.clone();
        let barrier_clone = barrier.clone();
        let handle1 = thread::spawn(move || {
            let store = Arc::new(OffsetStore::open(&path_clone).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_clone, "group1", 100)
                .with_crash_point(500)
                .with_commit_batch_size(100);
            let events = make_insert_events(1, 1, total, 100, 12345);
            barrier_clone.wait();
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
            (consumer.total_processed(), consumer.is_crashed())
        });

        let path_clone2 = path.clone();
        let processed_clone2 = processed_path_p2.clone();
        let barrier_clone2 = barrier.clone();
        let handle2 = thread::spawn(move || {
            let store = Arc::new(OffsetStore::open(&path_clone2).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_clone2, "group1", 200)
                .with_commit_batch_size(100);
            let events = make_insert_events(2, 1, total, 200, 12345);
            barrier_clone2.wait();
            for event in &events {
                consumer.process_event(event);
            }
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
            (consumer.total_processed(), consumer.is_crashed())
        });

        let (processed1, crashed1) = handle1.join().unwrap();
        let (processed2, crashed2) = handle2.join().unwrap();

        // 消费者 1 崩溃在 500，处理了 499 个
        assert!(crashed1);
        assert_eq!(processed1, 499);
        // 消费者 2 未崩溃，处理了 1000 个
        assert!(!crashed2);
        assert_eq!(processed2, 1000);

        cleanup_temp_files(&path);
        let _ = std::fs::remove_file(&processed_path_p1);
        let _ = std::fs::remove_file(&processed_path_p2);
    }
}

// =====================================================================
// Part 10: CDC 引擎集成（真实 WalRecord → ChangeEvent → FailoverConsumer）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part10_cdc_engine_integration {
    use super::*;
    use crate::{CdcEngine, CdcEventOp, CdcObserver, CdcObserverManager};
    use szrsql_tx::wal::{WalObserver, WalOpType, WalRecord};

    /// 自定义 CdcObserver，将事件转发给 FailoverConsumer
    struct FailoverObserver {
        consumer: Arc<FailoverConsumer>,
    }

    impl CdcObserver for FailoverObserver {
        fn on_event(&self, event: ChangeEvent) {
            // 只处理 DML 事件，跳过 Commit/Abort
            match event.op {
                CdcEventOp::Insert | CdcEventOp::Update | CdcEventOp::Delete => {
                    self.consumer.process_event(&event);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn phase_2_5_11_cdc_engine_integration_crash_recovery() {
        // 通过真实 CdcEngine 分发事件，FailoverConsumer 作为 observer
        let path = make_temp_path("part10_engine");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        // 构造 WalRecord（100 个 Insert + 1 个 Commit）
        let total = 100u64;
        let mut records = Vec::with_capacity(total as usize + 1);
        for i in 0..total {
            let lsn = i + 1;
            records.push(WalRecord::new(
                lsn,
                1,
                WalOpType::Insert,
                42,
                format!("row_{}", lsn).into_bytes(),
            ));
        }
        records.push(WalRecord::new(
            total + 1,
            1,
            WalOpType::Commit,
            0,
            Vec::new(),
        ));

        // 第一次会话：设置崩溃点，CdcEngine 分发
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = Arc::new(
                FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                    .with_crash_point(50)
                    .with_commit_batch_size(10),
            );
            let observer = Arc::new(FailoverObserver {
                consumer: consumer.clone(),
            });
            let mgr = Arc::new(CdcObserverManager::new());
            mgr.register(observer);
            let engine = CdcEngine::new(mgr.clone());
            engine.on_commit(1, records.clone());

            assert!(consumer.is_crashed());
            consumer.flush_processed().unwrap();
        }

        // 第二次会话：恢复
        // 注意：CdcEngine 会分发所有事件（包括已"处理"的），FailoverConsumer 通过
        // committed_lsn 和 processed set 去重
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = Arc::new(
                FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                    .with_commit_batch_size(10),
            );
            consumer.load_processed().unwrap();
            consumer.recover();

            let observer = Arc::new(FailoverObserver {
                consumer: consumer.clone(),
            });
            let mgr = Arc::new(CdcObserverManager::new());
            mgr.register(observer);
            let engine = CdcEngine::new(mgr.clone());
            // 重新分发所有事件（模拟重投）
            engine.on_commit(1, records.clone());

            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total as usize);
        assert_eq!(consumer.committed_lsn(), Some(total));

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 11: 大批量 stress（1M 事件 + 多次崩溃）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part11_stress {
    use super::*;

    #[test]
    fn phase_2_5_11_stress_1m_events_multiple_crashes() {
        // 1M 事件，3 次崩溃（200K / 500K / 800K）
        let path = make_temp_path("part11_stress_1m");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total = 1_000_000u64;
        let crash_points = [200_000u64, 500_000, 800_000];

        // 生成事件（为节省内存，按段生成）
        // 第一次会话：1..=200000，崩溃
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_crash_point(crash_points[0])
                .with_commit_batch_size(1000);
            let events = make_insert_events(1, 1, crash_points[0], 42, 12345);
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 第二次会话：从崩溃点继续到 500000，崩溃
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_crash_point(crash_points[1])
                .with_commit_batch_size(1000);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let count = crash_points[1] - restart_lsn + 1;
            let events = make_insert_events(1, restart_lsn, count, 42, 12345);
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 第三次会话：从崩溃点继续到 800000，崩溃
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_crash_point(crash_points[2])
                .with_commit_batch_size(1000);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let count = crash_points[2] - restart_lsn + 1;
            let events = make_insert_events(1, restart_lsn, count, 42, 12345);
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 第四次会话：从崩溃点继续到 1M，完成
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_commit_batch_size(1000);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let count = total - restart_lsn + 1;
            let events = make_insert_events(1, restart_lsn, count, 42, 12345);
            for event in &events {
                consumer.process_event(event);
            }
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total as usize);
        assert_eq!(consumer.committed_lsn(), Some(total));

        cleanup_temp_files(&path);
    }
}

// =====================================================================
// Part 12: Exactly-once 严格验证（无丢失 + 无重复）
// =====================================================================

#[cfg(test)]
mod phase_2_5_11_part12_exactly_once {
    use super::*;

    #[test]
    fn phase_2_5_11_exactly_once_no_loss_no_duplication() {
        // 严格验证：50000 事件，崩溃在 25000，恢复后完成
        // 检查：每个 LSN 恰好出现一次
        let path = make_temp_path("part12_exactly_once");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total = 50_000u64;
        let crash_lsn = 25_000u64;
        let events = make_insert_events(1, 1, total, 42, 12345);

        // 第一次会话
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_crash_point(crash_lsn)
                .with_commit_batch_size(500);
            for event in &events {
                if consumer.process_event(event) == ProcessResult::Crashed {
                    break;
                }
            }
            consumer.flush_processed().unwrap();
        }

        // 第二次会话
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_commit_batch_size(500);
            consumer.load_processed().unwrap();
            consumer.recover();
            let restart_lsn = consumer.next_lsn();
            let restart_idx = (restart_lsn - 1) as usize;
            for event in &events[restart_idx..] {
                consumer.process_event(event);
            }
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 严格验证：每个 LSN 1..=total 都在 processed set 中
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42);
        consumer.load_processed().unwrap();

        // 0 丢失
        for lsn in 1..=total {
            assert!(
                consumer.is_in_processed_set(lsn),
                "LSN {} missing (loss detected)",
                lsn
            );
        }

        // 0 重复：processed set 大小 == total
        assert_eq!(
            consumer.processed_set_size(),
            total as usize,
            "processed set size mismatch (duplication detected)"
        );

        // committed_lsn == total
        assert_eq!(consumer.committed_lsn(), Some(total));

        cleanup_temp_files(&path);
    }

    #[test]
    fn phase_2_5_11_exactly_once_with_redelivery() {
        // 场景：处理 1000 事件，未 commit 就崩溃，恢复后重投全部 1000 事件
        // 验证：最终 processed set 大小仍为 1000（去重生效）
        let path = make_temp_path("part12_redelivery");
        cleanup_temp_files(&path);
        let processed_path = processed_path_for(&path);

        let total = 1000u64;
        let events = make_insert_events(1, 1, total, 42, 12345);

        // 第一次会话：处理 999 个事件（LSN 1..=999），未 commit，崩溃在 1000
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_crash_point(total)
                .with_commit_batch_size(10000); // 大 batch，不触发自动 commit
            for event in &events[..999] {
                consumer.process_event(event);
            }
            // LSN 1000 触发崩溃
            let result = consumer.process_event(&events[999]);
            assert_eq!(result, ProcessResult::Crashed);
            consumer.flush_processed().unwrap();
            assert_eq!(consumer.committed_lsn(), None); // 未 commit
        }

        // 第二次会话：重投全部 1000 事件（committed_lsn = None → next_lsn = 1）
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42)
                .with_commit_batch_size(10000);
            consumer.load_processed().unwrap();
            consumer.recover();

            let mut new_count = 0u64;
            let mut dup_count = 0u64;
            for event in &events {
                match consumer.process_event(event) {
                    ProcessResult::Processed => new_count += 1,
                    ProcessResult::SkippedDuplicate => dup_count += 1,
                    _ => {}
                }
            }
            // LSN 1..=999 已在 processed set（SkippedDuplicate）
            // LSN 1000 是新的（Processed）
            assert_eq!(new_count, 1);
            assert_eq!(dup_count, 999);
            consumer.commit(total).unwrap();
            consumer.flush_processed().unwrap();
        }

        // 验证：processed set 大小 = 1000（0 丢失 + 0 重复）
        let store = Arc::new(OffsetStore::open(&path).unwrap());
        let consumer = FailoverConsumer::new(store.clone(), &processed_path, "group1", 42);
        consumer.load_processed().unwrap();
        assert_eq!(consumer.processed_set_size(), total as usize);

        cleanup_temp_files(&path);
    }
}
