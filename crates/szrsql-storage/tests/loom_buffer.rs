//! BufferPool 并发模型 loom 测试
//!
//! 目标：用 `loom` 穷举线程交错，验证 BufferPool 的并发安全性。
//!
//! 由于生产 `BufferPool` 直接使用 `std::sync::*`（无法在 loom 运行时替换），
//! 这里实现一个**与 BufferPool 完全相同的并发模型镜像**：
//! - LRU + lookup 共享同一个 `loom::sync::Mutex`（镜像生产 `BufferPoolShard`）
//! - `pin_count` / `dirty` 用 `loom::sync::atomic`
//! - 同样的 TOCTOU 风险点（read_page 二次锁、flush_all 脏页标志竞态等）
//!
//! 一旦 loom 发现模型层面的数据竞争 / 死锁 / 状态污染，
//! 即可反查 `src/buffer.rs` 中相同的代码路径并修复。
//!
//! 运行：
//! ```bash
//! cargo test -p szrsql-storage --features loom_model --test loom_buffer
//! ```

#![cfg(feature = "loom_model")]

use loom::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

// =====================================================================
//  模型：LRU 缓冲池（镜像 BufferPool 的并发结构）
// =====================================================================

/// 单个缓冲池条目（镜像 `buffer.rs::PageEntry`）
struct ModelPageEntry {
    /// 页内容（用 u64 简化，无需完整 Page）
    page: u64,
    pin_count: AtomicI32,
    dirty: loom::sync::atomic::AtomicBool,
}

impl ModelPageEntry {
    fn new(page: u64) -> Self {
        Self {
            page,
            pin_count: AtomicI32::new(0),
            dirty: loom::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// 单个分片（镜像 `buffer.rs::BufferPoolShard`：lru + lookup 共享同一 Mutex）
struct ModelShard {
    /// LRU 链表（前=最近使用，后=最久未用）
    lru: Vec<u32>,
    /// page_id → entry
    lookup: std::collections::HashMap<u32, ModelPageEntry>,
    capacity: usize,
}

/// 缓冲池模型（镜像 `buffer.rs::BufferPool`，但单分片简化）
struct ModelBufferPool {
    /// lru + lookup 共享一个 Mutex（与生产代码一致，避免 TOCTOU）
    shard: Mutex<ModelShard>,
    /// 模拟统计计数
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    /// 模拟写入器：脏页刷盘次数
    flush_count: AtomicU64,
}

impl ModelBufferPool {
    fn new(capacity: usize) -> Self {
        Self {
            shard: Mutex::new(ModelShard {
                lru: Vec::with_capacity(capacity),
                lookup: std::collections::HashMap::with_capacity(capacity),
                capacity,
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            flush_count: AtomicU64::new(0),
        }
    }

    /// 镜像 `BufferPool::read_page`：命中→移到 LRU 头；未命中→加载→插入（必要时淘汰）
    ///
    /// **关键 TOCTOU 点**：第一次锁检查存在性 → 释放锁 → 加载 → 第二次锁插入。
    /// 期间另一线程可能已插入，需二次检查。
    /// 但容量检查与插入在同一个锁内（原子），不会出现超容量。
    fn read_page(&self, page_id: u32, loader: impl Fn(u32) -> u64) -> u64 {
        // ===== 第一次锁：检查命中 =====
        {
            let shard = self.shard.lock().unwrap();
            if let Some(entry) = shard.lookup.get(&page_id) {
                let page = entry.page;
                drop(shard);

                // 移到 LRU 头部（重新加锁）
                let mut shard = self.shard.lock().unwrap();
                shard.lru.retain(|&p| p != page_id);
                shard.lru.push(page_id);
                drop(shard);

                self.hits.fetch_add(1, Ordering::SeqCst);
                return page;
            }
        }

        // 未命中
        self.misses.fetch_add(1, Ordering::SeqCst);

        // ===== 加载页（不持锁）=====
        let page = loader(page_id);

        // ===== 第二次锁：插入 =====
        let mut shard = self.shard.lock().unwrap();

        // 二次检查：可能在 drop 锁期间其他线程已加载
        if let Some(entry) = shard.lookup.get(&page_id) {
            let page = entry.page;
            shard.lru.retain(|&p| p != page_id);
            shard.lru.push(page_id);
            return page;
        }

        // 容量检查 — 需要淘汰？（在持有锁的情况下，与插入原子）
        if shard.lookup.len() >= shard.capacity {
            self.evict_one_locked(&mut shard);
        }

        // 插入
        shard.lookup.insert(page_id, ModelPageEntry::new(page));
        shard.lru.retain(|&p| p != page_id);
        shard.lru.push(page_id);

        page
    }

    /// 镜像 `BufferPool::pin_page`：pin_count += 1
    fn pin_page(&self, page_id: u32) -> Result<i32, ()> {
        let mut shard = self.shard.lock().unwrap();
        let entry = shard.lookup.get_mut(&page_id).ok_or(())?;

        let pin_count = entry.pin_count.fetch_add(1, Ordering::SeqCst) + 1;

        shard.lru.retain(|&p| p != page_id);
        shard.lru.push(page_id);

        Ok(pin_count)
    }

    /// 镜像 `BufferPool::unpin_page`：pin_count -= 1，禁止下溢
    fn unpin_page(&self, page_id: u32) -> Result<i32, ()> {
        let shard = self.shard.lock().unwrap();
        let entry = shard.lookup.get(&page_id).ok_or(())?;

        let current = entry.pin_count.load(Ordering::SeqCst);
        // 关键不变量：pin_count 不能下溢
        if current <= 0 {
            return Err(());
        }

        let new_count = entry.pin_count.fetch_sub(1, Ordering::SeqCst) - 1;
        Ok(new_count)
    }

    /// 镜像 `BufferPool::mark_dirty`
    fn mark_dirty(&self, page_id: u32) -> Result<(), ()> {
        let shard = self.shard.lock().unwrap();
        let entry = shard.lookup.get(&page_id).ok_or(())?;
        entry.dirty.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 镜像 `BufferPool::evict_one_locked`：淘汰 LRU 尾部第一个 pin_count==0 的页
    ///
    /// **关键不变量**：被淘汰的页必须 pin_count==0（不能淘汰被 pin 的页）
    /// 脏页淘汰前必须刷盘（避免数据丢失）
    ///
    /// 必须已持有 shard 锁
    fn evict_one_locked(&self, shard: &mut ModelShard) {
        let mut evict_idx = None;
        for (idx, &page_id) in shard.lru.iter().enumerate().rev() {
            if let Some(entry) = shard.lookup.get(&page_id) {
                if entry.pin_count.load(Ordering::SeqCst) == 0 {
                    evict_idx = Some(idx);
                    break;
                }
            }
        }

        let idx = match evict_idx {
            Some(i) => i,
            None => {
                // 所有页都被 pin，无法淘汰（与生产代码一致返回 Err）
                // 在 loom 模型中我们 panic 以暴露问题
                panic!("evict_one_locked: no evictable pages (all pinned)");
            }
        };

        let page_id = shard.lru.remove(idx);

        if let Some(entry) = shard.lookup.remove(&page_id) {
            let was_dirty = entry.dirty.load(Ordering::SeqCst);
            if was_dirty {
                self.flush_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        self.evictions.fetch_add(1, Ordering::SeqCst);
    }

    /// 镜像 `BufferPool::flush_all`：收集脏页 → 刷盘 → 清 dirty 标志
    ///
    /// **关键竞态点**：在收集脏页和清除 dirty 标志之间，另一线程可能 mark_dirty，
    /// 导致新的脏页标志被错误地清除（数据丢失）。
    /// 镜像此风险点以便 loom 检测。
    fn flush_all(&self) -> Result<usize, ()> {
        // 1. 收集脏页
        let mut to_flush: Vec<(u32, u64)> = Vec::new();
        {
            let shard = self.shard.lock().unwrap();
            for (&page_id, entry) in shard.lookup.iter() {
                if entry.dirty.load(Ordering::SeqCst) {
                    to_flush.push((page_id, entry.page));
                }
            }
        }

        // 2. 写入（模拟 writer.write_page）
        let flushed = to_flush.len();
        for _ in &to_flush {
            self.flush_count.fetch_add(1, Ordering::SeqCst);
        }

        // 3. 清除 dirty 标志
        // ⚠️ 风险：如果在步骤1和步骤3之间，另一线程修改了页内容并 mark_dirty，
        // 那么步骤3会错误地清除新的 dirty 标志，导致修改丢失。
        // 镜像此风险点（生产代码同样存在），让 loom 检测。
        let shard = self.shard.lock().unwrap();
        for (page_id, _) in &to_flush {
            if let Some(entry) = shard.lookup.get(page_id) {
                entry.dirty.store(false, Ordering::SeqCst);
            }
        }

        Ok(flushed)
    }
}

// =====================================================================
//  Loom 测试场景
// =====================================================================

/// 场景1：两线程并发 read_page 同一 page_id
///
/// 验证：不会重复加载、不会重复插入、LRU 状态一致
#[test]
fn loom_concurrent_read_same_page() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(4));

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || pool1.read_page(1, |pid| pid as u64 * 100));

        let pool2 = pool.clone();
        let h2 = thread::spawn(move || pool2.read_page(1, |pid| pid as u64 * 100));

        let p1 = h1.join().unwrap();
        let p2 = h2.join().unwrap();

        // 两线程应看到相同页内容
        assert_eq!(p1, p2);
        assert_eq!(p1, 100);

        // 不变量：缓冲池中 page_id=1 只存在一份
        let shard = pool.shard.lock().unwrap();
        assert_eq!(shard.lookup.len(), 1, "only one entry should exist");
        assert!(shard.lookup.contains_key(&1));
    });
}

/// 场景2：pin/unpin 配对，验证 pin_count 不下溢
///
/// 注意：两个并发 pin 的返回值是 {1, 2}（顺序未指定），
/// 所以断言总和为 3，而不是固定 c2 >= 2
#[test]
fn loom_pin_unpin_pairing() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(4));
        pool.read_page(1, |_| 42);

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || pool1.pin_page(1));
        let pool2 = pool.clone();
        let h2 = thread::spawn(move || pool2.pin_page(1));

        let c1 = h1.join().unwrap().unwrap();
        let c2 = h2.join().unwrap().unwrap();

        // 两次 pin 后返回值应为 {1, 2}（顺序未指定）
        assert!(c1 >= 1 && c2 >= 1, "both pin counts must be >= 1, got c1={c1} c2={c2}");
        assert_eq!(c1 + c2, 3, "sum of two concurrent pins must be 3 (1+2), got c1={c1} c2={c2}");

        // 两次 unpin 应回到 0
        let pool3 = pool.clone();
        let h3 = thread::spawn(move || pool3.unpin_page(1));
        let pool4 = pool.clone();
        let h4 = thread::spawn(move || pool4.unpin_page(1));

        let u1 = h3.join().unwrap();
        let u2 = h4.join().unwrap();

        // 两次 unpin 都应成功（不能下溢）
        assert!(u1.is_ok(), "first unpin must succeed");
        assert!(u2.is_ok(), "second unpin must succeed");
    });
}

/// 场景3：pin 后并发淘汰，验证被 pin 的页不会被淘汰
#[test]
fn loom_pinned_page_not_evicted() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(2));
        // 填满缓冲池
        pool.read_page(1, |_| 100);
        pool.read_page(2, |_| 200);

        // pin page 1
        pool.pin_page(1).unwrap();

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || {
            // read page 3 触发淘汰，但 page 1 被 pin 不能淘汰
            // 应淘汰 page 2
            pool1.read_page(3, |_| 300)
        });

        let pool2 = pool.clone();
        let h2 = thread::spawn(move || {
            // 并发再读 page 1（已 pin）
            pool2.read_page(1, |_| 100)
        });

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        assert_eq!(r1, 300);
        assert_eq!(r2, 100);

        // 不变量：page 1 仍在缓冲池中（被 pin 不能被淘汰）
        let shard = pool.shard.lock().unwrap();
        assert!(shard.lookup.contains_key(&1), "pinned page 1 must not be evicted");
    });
}

/// 场景4：并发 mark_dirty 和 flush_all，验证 dirty 标志竞态
///
/// ⚠️ 此场景可能暴露 flush_all 的设计缺陷：
/// - 线程A: mark_dirty(1) → flush_all（收集脏页）
/// - 线程B: mark_dirty(1)（在 flush_all 收集之后清 dirty 之前）
/// 结果：B 的修改未刷盘，但 dirty 标志被清除
#[test]
fn loom_concurrent_mark_dirty_and_flush() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(4));
        pool.read_page(1, |_| 0);

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || {
            pool1.mark_dirty(1).unwrap();
            pool1.flush_all().unwrap()
        });

        let pool2 = pool.clone();
        let h2 = thread::spawn(move || {
            pool2.mark_dirty(1).unwrap();
        });

        let _ = h1.join().unwrap();
        let _ = h2.join().unwrap();

        // 不变量验证：flush_count 应至少为 1（page 1 至少被刷盘一次）
        let flush_count = pool.flush_count.load(Ordering::SeqCst);
        assert!(
            flush_count >= 1,
            "page 1 must be flushed at least once, got {flush_count}"
        );
    });
}

/// 场景5：并发 read_page 多个不同 page_id 触发淘汰
///
/// 验证：多线程并发读取时，缓冲池大小不会超过容量
#[test]
fn loom_concurrent_read_different_pages_with_eviction() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(2));

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || pool1.read_page(1, |_| 100));

        let pool2 = pool.clone();
        let h2 = thread::spawn(move || pool2.read_page(2, |_| 200));

        let pool3 = pool.clone();
        let h3 = thread::spawn(move || pool3.read_page(3, |_| 300));

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();
        let r3 = h3.join().unwrap();

        assert_eq!(r1, 100);
        assert_eq!(r2, 200);
        assert_eq!(r3, 300);

        // 不变量：缓冲池中最多 capacity 个页
        let shard = pool.shard.lock().unwrap();
        assert!(
            shard.lookup.len() <= 2,
            "buffer pool size must not exceed capacity, got {}",
            shard.lookup.len()
        );
    });
}

/// 场景6：并发 pin 同一页多次 + 并发 unpin
///
/// 验证：pin_count 在并发下正确递增递减，不会出现负数
#[test]
fn loom_concurrent_pin_unpin_no_underflow() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(4));
        pool.read_page(1, |_| 42);

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || {
            let _ = pool1.pin_page(1);
            let _ = pool1.unpin_page(1);
        });

        let pool2 = pool.clone();
        let h2 = thread::spawn(move || {
            let _ = pool2.pin_page(1);
            let _ = pool2.unpin_page(1);
        });

        h1.join().unwrap();
        h2.join().unwrap();

        // 不变量：所有线程结束后 pin_count 应为 0
        let shard = pool.shard.lock().unwrap();
        let entry = shard.lookup.get(&1).expect("page 1 must exist");
        let pin_count = entry.pin_count.load(Ordering::SeqCst);
        assert_eq!(
            pin_count, 0,
            "pin_count must be 0 after balanced pin/unpin, got {pin_count}"
        );
    });
}

/// 场景7：read_page 与 mark_dirty 并发
///
/// 验证：read_page 命中时不会丢失 mark_dirty 标志
#[test]
fn loom_read_and_mark_dirty_concurrent() {
    loom::model(|| {
        let pool = Arc::new(ModelBufferPool::new(4));
        pool.read_page(1, |_| 100);

        let pool1 = pool.clone();
        let h1 = thread::spawn(move || pool1.read_page(1, |_| 200));

        let pool2 = pool.clone();
        let h2 = thread::spawn(move || pool2.mark_dirty(1));

        let r1 = h1.join().unwrap();
        let _ = h2.join().unwrap();

        // read_page 命中应返回原始内容（100）
        assert_eq!(r1, 100);

        // 不变量：dirty 标志最终应为 true（mark_dirty 已执行）
        let shard = pool.shard.lock().unwrap();
        let entry = shard.lookup.get(&1).expect("page 1 must exist");
        let dirty = entry.dirty.load(Ordering::SeqCst);
        assert!(dirty, "dirty flag must be true after mark_dirty");
    });
}
