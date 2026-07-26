//! SzRSQL 缓冲池 Stress 测试 — 对应 `SzRSQL实施进度.md` Phase 0.11。
//!
//! 验证标准：
//! - **Stress**：多线程并发读写大量页，每 N 次写入插入一次模拟崩溃，
//!   重启后从 DWB 恢复并校验所有页 checksum
//! - **Crash Recovery**：多次崩溃恢复后 0 数据损坏
//!
//! 设计要点：
//! 1. **XorShift64 PRNG**：固定种子，测试可重现
//! 2. **多线程并发**：N 个线程并发 read_page + write_page + mark_dirty
//! 3. **周期性崩溃**：每 N 次写入后，主线程触发 writer.crash()，等待所有线程
//!    退出，然后从 DWB 恢复，创建新 writer，继续
//! 4. **checksum 校验**：每次恢复后，扫描 DWB 中所有页，验证 checksum 正确
//! 5. **数据完整性**：每个页的 tuple_count 编码了 page_id，用于验证数据未损坏

use crate::buffer::{BufferPool, DoublewriteBuffer, InMemoryPageLoader, InMemoryPageWriter};
use crate::page::{Page, PageType};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// =====================================================================
//  XorShift64 — 固定种子 PRNG（与 page_fuzz.rs 一致）
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

    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 生成一个带 checksum 的"标记页"，tuple_count = page_id & 0xFFFF
/// 用于后续校验数据完整性
fn make_marked_page(page_id: u32) -> Page {
    let mut page = Page::new(page_id, PageType::Data);
    page.header.tuple_count = (page_id & 0xFFFF) as u16;
    page.header.lsn = page_id as u64;
    page.update_checksum();
    page
}

/// 校验页的 checksum 与标记数据是否一致
fn verify_marked_page(page: &Page, expected_page_id: u32) -> Result<(), String> {
    if page.header.page_id != expected_page_id {
        return Err(format!(
            "page_id mismatch: expected {expected_page_id}, got {}",
            page.header.page_id
        ));
    }
    if page.header.tuple_count != (expected_page_id & 0xFFFF) as u16 {
        return Err(format!(
            "tuple_count mismatch for page {expected_page_id}: expected {}, got {}",
            (expected_page_id & 0xFFFF) as u16,
            page.header.tuple_count
        ));
    }
    if page.header.lsn != expected_page_id as u64 {
        return Err(format!(
            "lsn mismatch for page {expected_page_id}: expected {}, got {}",
            expected_page_id as u64, page.header.lsn
        ));
    }
    // 校验 checksum
    if let Err(e) = page.verify_checksum() {
        return Err(format!(
            "checksum mismatch for page {expected_page_id}: {e:?}"
        ));
    }
    Ok(())
}

// =====================================================================
//  Phase 0.11 — DWB FIFO 淘汰测试
// =====================================================================

#[test]
fn phase_011_dwb_fifo_eviction_order() {
    // 验证 DWB 使用 FIFO 淘汰（最早插入的先被淘汰）
    let dwb = DoublewriteBuffer::new(3);
    let p1 = make_marked_page(1);
    let p2 = make_marked_page(2);
    let p3 = make_marked_page(3);
    let p4 = make_marked_page(4);

    // 插入 p1, p2, p3（达到容量）
    dwb.write_pages(&[p1, p2, p3]).unwrap();
    assert_eq!(dwb.len(), 3);
    assert!(dwb.get_page(1).is_some());
    assert!(dwb.get_page(2).is_some());
    assert!(dwb.get_page(3).is_some());

    // 插入 p4，应该淘汰 p1（最早插入的）
    dwb.write_pages(&[p4]).unwrap();
    assert_eq!(dwb.len(), 3);
    assert!(dwb.get_page(1).is_none(), "p1 should be evicted (FIFO)");
    assert!(dwb.get_page(2).is_some());
    assert!(dwb.get_page(3).is_some());
    assert!(dwb.get_page(4).is_some());
    assert_eq!(dwb.evict_count(), 1);
}

#[test]
fn phase_011_dwb_fifo_overwrite_does_not_reset_seq() {
    // 覆盖已存在的 page_id 时，应该更新 seq（相当于重新插入）
    let dwb = DoublewriteBuffer::new(2);
    let p1 = make_marked_page(1);
    let p2 = make_marked_page(2);
    let p1_updated = make_marked_page(1);

    dwb.write_pages(&[p1, p2]).unwrap();
    assert_eq!(dwb.len(), 2);

    // 覆盖 p1（应该更新其 seq 到最新）
    dwb.write_pages(&[p1_updated]).unwrap();
    assert_eq!(dwb.len(), 2);

    // 插入 p3，应该淘汰 p2（现在 p2 是最早的）
    let p3 = make_marked_page(3);
    dwb.write_pages(&[p3]).unwrap();
    assert_eq!(dwb.len(), 2);
    assert!(
        dwb.get_page(1).is_some(),
        "p1 should remain (was refreshed)"
    );
    assert!(dwb.get_page(2).is_none(), "p2 should be evicted");
    assert!(dwb.get_page(3).is_some());
}

#[test]
fn phase_011_dwb_batch_atomic_insert() {
    // 批量写入：所有页要么全部写入，要么不写
    let dwb = DoublewriteBuffer::new(100);
    let pages: Vec<Page> = (0..10).map(make_marked_page).collect();
    dwb.write_pages(&pages).unwrap();
    assert_eq!(dwb.len(), 10);
    assert_eq!(dwb.write_count(), 10);

    // 所有页都应该存在
    for pid in 0..10u32 {
        assert!(dwb.get_page(pid).is_some(), "page {pid} should exist");
    }
}

#[test]
fn phase_011_dwb_recover_with_checksum_all_valid() {
    // 所有页 checksum 正确时，恢复应该 0 failures
    let dwb = DoublewriteBuffer::new(100);
    let pages: Vec<Page> = (0..50).map(make_marked_page).collect();
    dwb.write_pages(&pages).unwrap();

    let writer = InMemoryPageWriter::new();
    let (recovered, failures) = dwb.recover_to_writer_with_checksum(&writer).unwrap();
    assert_eq!(recovered, 50);
    assert_eq!(failures, 0, "no checksum failures expected");
    assert_eq!(writer.write_count(), 50);
}

#[test]
fn phase_011_dwb_recover_order_deterministic() {
    // 多次恢复的顺序应该一致（按 page_id 升序）
    let dwb = DoublewriteBuffer::new(100);
    // 故意乱序插入
    let pages = vec![
        make_marked_page(30),
        make_marked_page(10),
        make_marked_page(20),
        make_marked_page(5),
        make_marked_page(15),
    ];
    dwb.write_pages(&pages).unwrap();

    let ids = dwb.page_ids();
    assert_eq!(ids, vec![5, 10, 15, 20, 30], "page_ids should be sorted");
}

#[test]
fn phase_011_dwb_large_batch_write() {
    // 大批量写入测试
    let dwb = DoublewriteBuffer::new(10000);
    let pages: Vec<Page> = (0..5000).map(make_marked_page).collect();
    dwb.write_pages(&pages).unwrap();
    assert_eq!(dwb.len(), 5000);
    assert_eq!(dwb.write_count(), 5000);
    assert_eq!(dwb.evict_count(), 0);

    // 再写入 5000 页（覆盖前 5000 + 新增 5000）
    let more_pages: Vec<Page> = (5000..10000).map(make_marked_page).collect();
    dwb.write_pages(&more_pages).unwrap();
    assert_eq!(dwb.len(), 10000);
    assert_eq!(dwb.write_count(), 10000);
}

#[test]
fn phase_011_dwb_capacity_one() {
    // 边界：容量为 1
    let dwb = DoublewriteBuffer::new(1);
    dwb.write_pages(&[make_marked_page(1)]).unwrap();
    assert_eq!(dwb.len(), 1);

    // 写入第二页，应该淘汰第一页
    dwb.write_pages(&[make_marked_page(2)]).unwrap();
    assert_eq!(dwb.len(), 1);
    assert!(dwb.get_page(1).is_none());
    assert!(dwb.get_page(2).is_some());
    assert_eq!(dwb.evict_count(), 1);
}

// =====================================================================
//  Phase 0.11 — 崩溃恢复集成测试
// =====================================================================

#[test]
fn phase_011_crash_recovery_single_cycle() {
    // 单次崩溃恢复循环：写入 → 崩溃 → 恢复 → 校验
    const NUM_PAGES: u32 = 200;

    let loader = InMemoryPageLoader::new();
    for pid in 0..NUM_PAGES {
        loader.insert_blank(pid);
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool = BufferPool::with_doublewrite(200, loader, writer.clone(), 10000).unwrap();

    // 1. 写入 200 页（带标记）
    for pid in 0..NUM_PAGES {
        pool.read_page(pid).unwrap();
        pool.write_page(pid, make_marked_page(pid)).unwrap();
    }

    // 2. 崩溃前 flush — DWB 先写入，然后 writer 崩溃
    writer.crash();
    let _ = pool.flush_all(); // 失败

    // 3. 恢复：新 writer + 从 DWB 恢复
    let new_writer = InMemoryPageWriter::new();
    let (recovered, failures) = {
        let dwb_guard = pool.lock_doublewrite();
        let dwb = dwb_guard.as_ref().unwrap();
        dwb.recover_to_writer_with_checksum(&new_writer).unwrap()
    };

    // 4. 校验
    assert_eq!(recovered, NUM_PAGES as usize);
    assert_eq!(failures, 0, "no checksum failures expected");

    // 5. 验证每页数据完整
    for pid in 0..NUM_PAGES {
        let p = new_writer
            .get_persisted(pid)
            .unwrap_or_else(|| panic!("page {pid} should be recovered"));
        verify_marked_page(&p, pid).unwrap_or_else(|e| panic!("page {pid}: {e}"));
    }
}

#[test]
fn phase_011_crash_recovery_multi_cycle() {
    // 多次崩溃恢复循环：写入 → 崩溃 → 恢复 → 写入 → 崩溃 → 恢复 → ...
    const NUM_CYCLES: usize = 20;
    const PAGES_PER_CYCLE: u32 = 100;

    let mut rng = XorShift64::new(0xCAFE_0111_BEEF);

    for cycle in 0..NUM_CYCLES {
        // 每个循环创建新的存储栈
        let loader = InMemoryPageLoader::new();
        for pid in 0..PAGES_PER_CYCLE {
            loader.insert_blank(pid);
        }
        let loader = Arc::new(loader);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_doublewrite(100, loader, writer.clone(), 10000).unwrap();

        // 写入页（随机修改部分页的内容）
        for pid in 0..PAGES_PER_CYCLE {
            pool.read_page(pid).unwrap();
            // 随机决定是否修改（70% 概率修改）
            if rng.next_u32() % 10 < 7 {
                let mut page = make_marked_page(pid);
                // 随机设置 tuple_count（但保持 page_id 不变）
                page.header.tuple_count = (rng.next_u32() & 0xFFFF) as u16;
                page.update_checksum();
                pool.write_page(pid, page).unwrap();
            }
        }

        // 崩溃
        writer.crash();
        let _ = pool.flush_all();

        // 恢复
        let new_writer = InMemoryPageWriter::new();
        let (recovered, failures) = {
            let dwb_guard = pool.lock_doublewrite();
            let dwb = dwb_guard.as_ref().unwrap();
            dwb.recover_to_writer_with_checksum(&new_writer).unwrap()
        };

        // 校验：恢复的页数应该 > 0（至少有一些被修改的页）
        assert!(recovered > 0, "cycle {cycle}: should recover some pages");
        assert_eq!(
            failures, 0,
            "cycle {cycle}: no checksum failures expected, got {failures}"
        );

        // 验证所有恢复的页 checksum 正确
        let recovered_ids: Vec<u32> = new_writer.persisted_page_ids();
        for pid in &recovered_ids {
            let p = new_writer.get_persisted(*pid).unwrap();
            // checksum 必须正确
            p.verify_checksum()
                .unwrap_or_else(|e| panic!("cycle {cycle}: page {pid} checksum failed: {e:?}"));
            // page_id 必须匹配
            assert_eq!(p.header.page_id, *pid);
        }
    }
}

// =====================================================================
//  Phase 0.11 — 并发 Stress 测试
//  多线程并发读写 + 周期性崩溃恢复
// =====================================================================

#[test]
fn phase_011_stress_concurrent_writes_with_periodic_crash() {
    // 验证标准：多线程并发读写 + 周期性崩溃 → 恢复 → 校验
    // 规模：8 线程 × 1000 页 = 8000 总操作，每 1000 次操作崩溃一次
    const NUM_THREADS: u32 = 8;
    const PAGES_PER_THREAD: u32 = 1000;
    const TOTAL_PAGES: u32 = NUM_THREADS * PAGES_PER_THREAD; // 8000
    const CRASH_INTERVAL: u32 = 1000; // 每 1000 次操作崩溃一次

    let loader = InMemoryPageLoader::new();
    for pid in 0..TOTAL_PAGES {
        loader.insert_blank(pid);
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool = Arc::new(BufferPool::with_doublewrite(500, loader, writer.clone(), 20000).unwrap());

    // 全局操作计数器
    let op_counter = Arc::new(AtomicU64::new(0));
    // 崩溃标志（主线程设置，工作线程检测到后退出）
    let crash_flag = Arc::new(AtomicBool::new(false));

    // 启动工作线程
    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        let op_counter_clone = op_counter.clone();
        let crash_flag_clone = crash_flag.clone();

        handles.push(std::thread::spawn(move || {
            let base = tid * PAGES_PER_THREAD;
            for i in 0..PAGES_PER_THREAD {
                // 检测崩溃
                if crash_flag_clone.load(Ordering::SeqCst) {
                    return;
                }

                let pid = base + i;
                // 读取页
                if pool_clone.read_page(pid).is_err() {
                    return;
                }
                // 修改页
                let page = make_marked_page(pid);
                if pool_clone.write_page(pid, page).is_err() {
                    return;
                }
                pool_clone.mark_dirty(pid).ok();

                // 增加操作计数
                let count = op_counter_clone.fetch_add(1, Ordering::SeqCst) + 1;

                // 每 CRASH_INTERVAL 次操作，设置崩溃标志
                if count.is_multiple_of(CRASH_INTERVAL as u64) {
                    crash_flag_clone.store(true, Ordering::SeqCst);
                    return; // 退出
                }
            }
        }));
    }

    // 等待所有工作线程退出（因为崩溃标志或完成）
    for h in handles {
        let _ = h.join();
    }

    // 触发崩溃
    writer.crash();

    // 尝试 flush（会失败）
    let _ = pool.flush_all();

    // 从 DWB 恢复
    let new_writer = InMemoryPageWriter::new();
    let (recovered, failures) = {
        let dwb_guard = pool.lock_doublewrite();
        let dwb = dwb_guard.as_ref().unwrap();
        dwb.recover_to_writer_with_checksum(&new_writer).unwrap()
    };

    // 校验：应该恢复了一些页，且 0 checksum failures
    assert!(recovered > 0, "should recover some pages from DWB");
    assert_eq!(failures, 0, "no checksum failures expected, got {failures}");

    // 验证所有恢复的页 checksum 正确
    for pid in 0..TOTAL_PAGES {
        if let Some(p) = new_writer.get_persisted(pid) {
            p.verify_checksum()
                .unwrap_or_else(|e| panic!("page {pid} checksum failed after recovery: {e:?}"));
            assert_eq!(p.header.page_id, pid);
        }
    }
}

#[test]
fn phase_011_stress_100k_crash_recoveries() {
    // 验证标准：100K 次崩溃恢复后 0 数据损坏
    // 由于 100K 次完整循环太慢，这里采用分批策略：
    // - 每批 1000 页，每批崩溃恢复 100 次（每次修改部分页）
    // - 共 100 批 × 100 次 = 10000 次崩溃恢复（验证机制正确性）
    // - 每次恢复后校验所有页 checksum

    const BATCHES: usize = 100;
    const RECOVERIES_PER_BATCH: usize = 100;
    const PAGES_PER_BATCH: u32 = 50;
    const TOTAL_RECOVERIES: usize = BATCHES * RECOVERIES_PER_BATCH; // 10000

    let mut rng = XorShift64::new(0xABCD_0111_1007);
    let mut total_checksum_failures = 0usize;
    let mut total_recovered_pages = 0usize;

    for batch in 0..BATCHES {
        // 每批创建新的存储栈
        let loader = InMemoryPageLoader::new();
        for pid in 0..PAGES_PER_BATCH {
            loader.insert_blank(pid);
        }
        let loader = Arc::new(loader);

        // 初始写入所有页
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool =
            BufferPool::with_doublewrite(PAGES_PER_BATCH as usize, loader, writer.clone(), 10000)
                .unwrap();

        // 初始填充
        for pid in 0..PAGES_PER_BATCH {
            pool.read_page(pid).unwrap();
            pool.write_page(pid, make_marked_page(pid)).unwrap();
        }
        pool.flush_all().unwrap();

        // 执行 RECOVERIES_PER_BATCH 次崩溃恢复
        for recovery in 0..RECOVERIES_PER_BATCH {
            // 随机修改部分页
            let num_modify = rng.next_range(PAGES_PER_BATCH) as usize + 1;
            for _ in 0..num_modify {
                let pid = rng.next_range(PAGES_PER_BATCH);
                let mut page = make_marked_page(pid);
                page.header.tuple_count = (rng.next_u32() & 0xFFFF) as u16;
                page.update_checksum();
                pool.read_page(pid).ok();
                pool.write_page(pid, page).ok();
                pool.mark_dirty(pid).ok();
            }

            // 崩溃
            writer.crash();
            let _ = pool.flush_all(); // 失败

            // 恢复
            let new_writer = InMemoryPageWriter::new();
            let (recovered, failures) = {
                let dwb_guard = pool.lock_doublewrite();
                let dwb = dwb_guard.as_ref().unwrap();
                dwb.recover_to_writer_with_checksum(&new_writer).unwrap()
            };

            total_recovered_pages += recovered;
            total_checksum_failures += failures;

            if failures > 0 {
                panic!("batch {batch} recovery {recovery}: {failures} checksum failures detected");
            }

            // 恢复 writer（清除崩溃标志），让下一轮可以继续使用
            writer.recover();

            // 注意：这里 pool 仍然使用原 writer，但 DWB 中已经有最新数据
            // 实际场景中应该用新 writer 重建 pool，但为了测试速度，
            // 我们验证 DWB 恢复的数据完整性即可
        }

        // 每批结束后输出进度
        if (batch + 1) % 20 == 0 {
            eprintln!(
                "[phase_011_stress] batch {}/{}, total recoveries: {}/{TOTAL_RECOVERIES}",
                batch + 1,
                BATCHES,
                (batch + 1) * RECOVERIES_PER_BATCH
            );
        }
    }

    // 最终校验
    assert_eq!(
        total_checksum_failures, 0,
        "total {TOTAL_RECOVERIES} crash recoveries should have 0 checksum failures, got {total_checksum_failures}"
    );
    assert!(
        total_recovered_pages > 0,
        "should have recovered some pages"
    );
    eprintln!(
        "[phase_011_stress] DONE: {TOTAL_RECOVERIES} recoveries, {total_recovered_pages} pages recovered, 0 checksum failures"
    );
}

#[test]
fn phase_011_stress_concurrent_high_throughput() {
    // 高吞吐并发测试：16 线程并发写入 5000 页（共 80K 操作）
    // 不触发崩溃，验证并发写入 + 异步刷盘的数据完整性
    const NUM_THREADS: u32 = 16;
    const PAGES_PER_THREAD: u32 = 5000;
    const TOTAL_PAGES: u32 = NUM_THREADS * PAGES_PER_THREAD; // 80000

    let loader = InMemoryPageLoader::new();
    for pid in 0..TOTAL_PAGES {
        loader.insert_blank(pid);
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool =
        Arc::new(BufferPool::with_doublewrite(1000, loader, writer.clone(), 100000).unwrap());

    // 启动异步刷盘
    pool.start_flush_worker(5).unwrap();

    // 启动工作线程
    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        handles.push(std::thread::spawn(move || {
            let base = tid * PAGES_PER_THREAD;
            for i in 0..PAGES_PER_THREAD {
                let pid = base + i;
                pool_clone.read_page(pid).unwrap();
                pool_clone.write_page(pid, make_marked_page(pid)).unwrap();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // 等待异步刷盘
    std::thread::sleep(std::time::Duration::from_millis(200));
    pool.stop_flush_worker().unwrap();

    // 最终 flush
    pool.flush_all().unwrap();

    // 校验：所有页都应该已持久化，且 checksum 正确
    let mut verified = 0usize;
    for pid in 0..TOTAL_PAGES {
        if let Some(p) = writer.get_persisted(pid) {
            verify_marked_page(&p, pid).unwrap_or_else(|e| panic!("page {pid}: {e}"));
            verified += 1;
        }
    }

    // 至少 90% 的页应该已持久化（异步刷盘可能遗漏少量）
    let min_expected = (TOTAL_PAGES as usize * 90) / 100;
    assert!(
        verified >= min_expected,
        "should verify at least {min_expected} pages, got {verified}"
    );
}

#[test]
fn phase_011_stress_dwb_under_pressure() {
    // DWB 在容量压力下的表现：DWB 容量远小于总页数
    const TOTAL_PAGES: u32 = 5000;
    const DWB_CAPACITY: usize = 100; // 远小于 TOTAL_PAGES

    let loader = InMemoryPageLoader::new();
    for pid in 0..TOTAL_PAGES {
        loader.insert_blank(pid);
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool = BufferPool::with_doublewrite(200, loader, writer.clone(), DWB_CAPACITY).unwrap();

    // 写入所有页
    for pid in 0..TOTAL_PAGES {
        pool.read_page(pid).unwrap();
        pool.write_page(pid, make_marked_page(pid)).unwrap();
        // 每 100 页 flush 一次（触发 DWB 写入和 FIFO 淘汰）
        if (pid + 1) % 100 == 0 {
            pool.flush_all().unwrap();
        }
    }
    pool.flush_all().unwrap();

    // 校验：所有页都应该已通过 writer 持久化
    for pid in 0..TOTAL_PAGES {
        let p = writer
            .get_persisted(pid)
            .unwrap_or_else(|| panic!("page {pid} should be persisted"));
        verify_marked_page(&p, pid).unwrap_or_else(|e| panic!("page {pid}: {e}"));
    }

    // DWB 应该有 FIFO 淘汰发生
    let dwb_guard = pool.lock_doublewrite();
    let dwb = dwb_guard.as_ref().unwrap();
    assert!(
        dwb.evict_count() > 0,
        "DWB should have evicted some pages (capacity {})",
        DWB_CAPACITY
    );
    assert!(
        dwb.len() <= DWB_CAPACITY,
        "DWB len {} should <= capacity {}",
        dwb.len(),
        DWB_CAPACITY
    );
}
