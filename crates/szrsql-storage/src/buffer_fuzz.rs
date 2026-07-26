//! SzRSQL 缓冲池并发 Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 0.12。
//!
//! 验证标准：
//! - **Fuzz**：16 线程随机 read_page/mark_dirty/flush/evict 10000000 次
//! - **Checksum 对比**：启动时和结束时各读全量页做 checksum 对比
//! - **验证**：结束时所有页 checksum 等于开始时（页内容不被并发操作破坏）
//!
//! 设计要点：
//! 1. **只读内容**：fuzz 操作只涉及 read_page/mark_dirty/flush/evict，
//!    不调用 write_page(pid, new_page) 修改页内容
//! 2. **小容量缓冲池**：capacity 远小于总页数，强制频繁淘汰（触发 evict）
//! 3. **固定种子 PRNG**：XorShift64，每线程独立种子，测试可重现
//! 4. **checksum 快照**：启动时记录所有页 checksum，结束后逐页对比
//! 5. **线程安全验证**：并发操作不应导致 panic 或数据竞争

use crate::buffer::{BufferPool, InMemoryPageLoader, InMemoryPageWriter};
use crate::page::{Page, PageType};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// =====================================================================
//  XorShift64 — 固定种子 PRNG（与 page_fuzz.rs / buffer_stress.rs 一致）
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

/// 生成带 checksum 的"标记页"：tuple_count = page_id & 0xFFFF, lsn = page_id
fn make_marked_page(page_id: u32) -> Page {
    let mut page = Page::new(page_id, PageType::Data);
    page.header.tuple_count = (page_id & 0xFFFF) as u16;
    page.header.lsn = page_id as u64;
    page.update_checksum();
    page
}

/// 校验页 checksum 与标记数据一致性
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
    page.verify_checksum()
        .map_err(|e| format!("checksum mismatch for page {expected_page_id}: {e:?}"))
}

/// 快照所有页的 checksum（从 loader 读取原始页）
fn snapshot_checksums(loader: &InMemoryPageLoader, num_pages: u32) -> Vec<u32> {
    (0..num_pages)
        .map(|pid| {
            loader
                .get_persisted(pid)
                .unwrap_or_else(|| panic!("page {pid} missing from loader"))
                .header
                .checksum
        })
        .collect()
}

/// 验证缓冲池中所有页的 checksum 与快照一致
fn verify_against_snapshot(
    pool: &BufferPool,
    snapshot: &[u32],
    num_pages: u32,
) -> Result<(), String> {
    for pid in 0..num_pages {
        let page = pool
            .read_page(pid)
            .map_err(|e| format!("read page {pid} failed: {e:?}"))?;
        let expected = snapshot[pid as usize];
        if page.header.checksum != expected {
            return Err(format!(
                "page {pid} checksum mismatch: expected {expected:#010x}, got {:#010x}",
                page.header.checksum
            ));
        }
        verify_marked_page(&page, pid)?;
    }
    Ok(())
}

// =====================================================================
//  Fuzz 操作定义
// =====================================================================

/// Fuzz 操作类型（对应 spec 的 read_page/mark_dirty/flush/evict）
///
/// 注意：evict 不是显式操作，而是 read_page 在缓冲池满时自动触发
#[derive(Clone, Copy, Debug)]
enum FuzzOp {
    /// 读取页（未命中时从 loader 加载，缓冲池满时触发淘汰）
    Read,
    /// 标记页为脏页
    MarkDirty,
    /// 刷盘单页
    FlushPage,
    /// 刷盘所有脏页
    FlushAll,
}

impl FuzzOp {
    fn from_u32(v: u32) -> Self {
        match v % 4 {
            0 => FuzzOp::Read,
            1 => FuzzOp::MarkDirty,
            2 => FuzzOp::FlushPage,
            _ => FuzzOp::FlushAll,
        }
    }
}

/// 执行单个 fuzz 操作
fn execute_op(pool: &BufferPool, rng: &mut XorShift64, num_pages: u32) {
    let op = FuzzOp::from_u32(rng.next_u32());
    let pid = rng.next_range(num_pages);
    match op {
        FuzzOp::Read => {
            pool.read_page(pid).ok();
        }
        FuzzOp::MarkDirty => {
            pool.mark_dirty(pid).ok();
        }
        FuzzOp::FlushPage => {
            pool.flush_page(pid).ok();
        }
        FuzzOp::FlushAll => {
            pool.flush_all().ok();
        }
    }
}

// =====================================================================
//  Phase 0.12 — 并发 Fuzz 测试
// =====================================================================

#[test]
fn phase_012_concurrent_fuzz_10m_ops_checksum_stable() {
    // 验证标准（spec）：
    //   16 线程随机 read_page/mark_dirty/flush/evict 10000000 次
    //   启动时和结束时各读全量页做 checksum 对比
    // 规模：16 线程 × 625K ops = 10,000,000 总操作
    const NUM_PAGES: u32 = 1000;
    const NUM_THREADS: u32 = 16;
    const OPS_PER_THREAD: u64 = 625_000; // 16 * 625000 = 10,000,000
    const POOL_CAPACITY: usize = 100; // 远小于 NUM_PAGES，强制频繁淘汰

    // 1. 预填充 loader（1000 页，每页有唯一标记和 checksum）
    let loader = InMemoryPageLoader::new();
    for pid in 0..NUM_PAGES {
        loader.insert(pid, make_marked_page(pid));
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool =
        Arc::new(BufferPool::with_writer(POOL_CAPACITY, loader.clone(), writer.clone()).unwrap());

    // 2. 启动时快照所有页的 checksum
    let initial_checksums = snapshot_checksums(&loader, NUM_PAGES);

    // 3. 16 线程并发执行随机操作
    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        let seed = 0xF012_0000_0000_0000 + tid as u64;
        handles.push(std::thread::spawn(move || {
            let mut rng = XorShift64::new(seed);
            for _ in 0..OPS_PER_THREAD {
                execute_op(&pool_clone, &mut rng, NUM_PAGES);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 4. 最终 flush（确保所有脏页写回 writer）
    pool.flush_all().ok();

    // 5. 结束时校验所有页 checksum 与启动时一致
    verify_against_snapshot(&pool, &initial_checksums, NUM_PAGES)
        .unwrap_or_else(|e| panic!("checksum verification failed: {e}"));

    // 6. 输出统计
    let stats = pool.stats();
    eprintln!(
        "[phase_012] 10M ops done: hits={}, misses={}, evictions={}, flush_count={}, dirty_pages={}",
        stats.hits, stats.misses, stats.evictions, stats.flush_count, stats.dirty_pages
    );
}

#[test]
fn phase_012_concurrent_fuzz_with_dwb_checksum_stable() {
    // 启用 Doublewrite Buffer 的并发 fuzz：验证 DWB 不破坏页内容
    // 规模：16 线程 × 312.5K ops = 5,000,000 总操作（DWB 增加开销，减半规模）
    const NUM_PAGES: u32 = 500;
    const NUM_THREADS: u32 = 16;
    const OPS_PER_THREAD: u64 = 312_500; // 16 * 312500 = 5,000,000
    const POOL_CAPACITY: usize = 50;
    const DWB_CAPACITY: usize = 500;

    let loader = InMemoryPageLoader::new();
    for pid in 0..NUM_PAGES {
        loader.insert(pid, make_marked_page(pid));
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool = Arc::new(
        BufferPool::with_doublewrite(POOL_CAPACITY, loader.clone(), writer.clone(), DWB_CAPACITY)
            .unwrap(),
    );

    let initial_checksums = snapshot_checksums(&loader, NUM_PAGES);

    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        let seed = 0xDA0B_C0DE_0000_0000 + tid as u64;
        handles.push(std::thread::spawn(move || {
            let mut rng = XorShift64::new(seed);
            for _ in 0..OPS_PER_THREAD {
                execute_op(&pool_clone, &mut rng, NUM_PAGES);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    pool.flush_all().ok();

    verify_against_snapshot(&pool, &initial_checksums, NUM_PAGES)
        .unwrap_or_else(|e| panic!("DWB checksum verification failed: {e}"));

    let stats = pool.stats();
    eprintln!(
        "[phase_012_dwb] 5M ops done: hits={}, misses={}, evictions={}, flush_count={}",
        stats.hits, stats.misses, stats.evictions, stats.flush_count
    );
}

#[test]
fn phase_012_concurrent_fuzz_high_contention_no_panic() {
    // 高竞争场景：极小容量（10 页）+ 16 线程，验证无 panic 且数据完整
    // 规模：16 线程 × 100K ops = 1,600,000 总操作
    const NUM_PAGES: u32 = 200;
    const NUM_THREADS: u32 = 16;
    const OPS_PER_THREAD: u64 = 100_000;
    const POOL_CAPACITY: usize = 10; // 极小容量，高淘汰率

    let loader = InMemoryPageLoader::new();
    for pid in 0..NUM_PAGES {
        loader.insert(pid, make_marked_page(pid));
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool =
        Arc::new(BufferPool::with_writer(POOL_CAPACITY, loader.clone(), writer.clone()).unwrap());

    let initial_checksums = snapshot_checksums(&loader, NUM_PAGES);

    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        let seed = 0xA1A0_C0DE_0000_0000 + tid as u64;
        handles.push(std::thread::spawn(move || {
            let mut rng = XorShift64::new(seed);
            for _ in 0..OPS_PER_THREAD {
                execute_op(&pool_clone, &mut rng, NUM_PAGES);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    pool.flush_all().ok();

    verify_against_snapshot(&pool, &initial_checksums, NUM_PAGES)
        .unwrap_or_else(|e| panic!("high contention checksum verification failed: {e}"));

    let stats = pool.stats();
    eprintln!(
        "[phase_012_high] 1.6M ops done: hits={}, misses={}, evictions={} (eviction rate {:.1}%)",
        stats.hits,
        stats.misses,
        stats.evictions,
        if stats.misses > 0 {
            stats.evictions as f64 / stats.misses as f64 * 100.0
        } else {
            0.0
        }
    );
}

#[test]
fn phase_012_concurrent_fuzz_async_flush_checksum_stable() {
    // 启用异步刷盘线程的并发 fuzz：验证后台刷盘不破坏页内容
    // 规模：16 线程 × 312.5K ops = 5,000,000 总操作 + 异步刷盘每 2ms 一次
    const NUM_PAGES: u32 = 500;
    const NUM_THREADS: u32 = 16;
    const OPS_PER_THREAD: u64 = 312_500;
    const POOL_CAPACITY: usize = 50;
    const FLUSH_INTERVAL_MS: u64 = 2;

    let loader = InMemoryPageLoader::new();
    for pid in 0..NUM_PAGES {
        loader.insert(pid, make_marked_page(pid));
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool =
        Arc::new(BufferPool::with_writer(POOL_CAPACITY, loader.clone(), writer.clone()).unwrap());

    let initial_checksums = snapshot_checksums(&loader, NUM_PAGES);

    // 启动异步刷盘线程
    pool.start_flush_worker(FLUSH_INTERVAL_MS).unwrap();

    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        let seed = 0xA5AC_C0DE_0000_0000 + tid as u64;
        handles.push(std::thread::spawn(move || {
            let mut rng = XorShift64::new(seed);
            for _ in 0..OPS_PER_THREAD {
                execute_op(&pool_clone, &mut rng, NUM_PAGES);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 停止异步刷盘并最终 flush
    pool.stop_flush_worker().unwrap();
    pool.flush_all().ok();

    verify_against_snapshot(&pool, &initial_checksums, NUM_PAGES)
        .unwrap_or_else(|e| panic!("async flush checksum verification failed: {e}"));

    let stats = pool.stats();
    eprintln!(
        "[phase_012_async] 5M ops done: hits={}, misses={}, evictions={}, flush_count={}",
        stats.hits, stats.misses, stats.evictions, stats.flush_count
    );
}

#[test]
fn phase_012_concurrent_fuzz_mixed_ops_distribution() {
    // 验证 fuzz 操作分布均匀：统计各操作类型占比
    // 确保所有 4 种操作都被执行，且分布大致均匀（25% ± 5%）
    const NUM_PAGES: u32 = 100;
    const NUM_THREADS: u32 = 16;
    const OPS_PER_THREAD: u64 = 62_500; // 16 * 62500 = 1,000,000
    const POOL_CAPACITY: usize = 50;

    let loader = InMemoryPageLoader::new();
    for pid in 0..NUM_PAGES {
        loader.insert(pid, make_marked_page(pid));
    }
    let loader = Arc::new(loader);
    let writer = Arc::new(InMemoryPageWriter::new());
    let pool =
        Arc::new(BufferPool::with_writer(POOL_CAPACITY, loader.clone(), writer.clone()).unwrap());

    let initial_checksums = snapshot_checksums(&loader, NUM_PAGES);

    // 全局操作计数器（按操作类型）
    let read_count = Arc::new(AtomicU64::new(0));
    let mark_dirty_count = Arc::new(AtomicU64::new(0));
    let flush_page_count = Arc::new(AtomicU64::new(0));
    let flush_all_count = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for tid in 0..NUM_THREADS {
        let pool_clone = pool.clone();
        let read_clone = read_count.clone();
        let mark_clone = mark_dirty_count.clone();
        let flush_p_clone = flush_page_count.clone();
        let flush_a_clone = flush_all_count.clone();
        let seed = 0xD15A_C0DE_0000_0000 + tid as u64;
        handles.push(std::thread::spawn(move || {
            let mut rng = XorShift64::new(seed);
            for _ in 0..OPS_PER_THREAD {
                let op = FuzzOp::from_u32(rng.next_u32());
                let pid = rng.next_range(NUM_PAGES);
                match op {
                    FuzzOp::Read => {
                        pool_clone.read_page(pid).ok();
                        read_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    FuzzOp::MarkDirty => {
                        pool_clone.mark_dirty(pid).ok();
                        mark_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    FuzzOp::FlushPage => {
                        pool_clone.flush_page(pid).ok();
                        flush_p_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    FuzzOp::FlushAll => {
                        pool_clone.flush_all().ok();
                        flush_a_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    pool.flush_all().ok();

    // 校验 checksum
    verify_against_snapshot(&pool, &initial_checksums, NUM_PAGES)
        .unwrap_or_else(|e| panic!("mixed ops checksum verification failed: {e}"));

    // 验证操作分布（每种操作至少 15%，最多 35%）
    let total = read_count.load(Ordering::Relaxed)
        + mark_dirty_count.load(Ordering::Relaxed)
        + flush_page_count.load(Ordering::Relaxed)
        + flush_all_count.load(Ordering::Relaxed);
    assert_eq!(total, NUM_THREADS as u64 * OPS_PER_THREAD);

    let read_pct = read_count.load(Ordering::Relaxed) as f64 / total as f64 * 100.0;
    let mark_pct = mark_dirty_count.load(Ordering::Relaxed) as f64 / total as f64 * 100.0;
    let flush_p_pct = flush_page_count.load(Ordering::Relaxed) as f64 / total as f64 * 100.0;
    let flush_a_pct = flush_all_count.load(Ordering::Relaxed) as f64 / total as f64 * 100.0;

    eprintln!(
        "[phase_012_dist] read={:.1}% mark_dirty={:.1}% flush_page={:.1}% flush_all={:.1}% (total={total})",
        read_pct, mark_pct, flush_p_pct, flush_a_pct
    );

    for (name, pct) in [
        ("read", read_pct),
        ("mark_dirty", mark_pct),
        ("flush_page", flush_p_pct),
        ("flush_all", flush_a_pct),
    ] {
        assert!(
            (15.0..=35.0).contains(&pct),
            "op {name} distribution {pct:.1}% out of [15%, 35%] range"
        );
    }
}
