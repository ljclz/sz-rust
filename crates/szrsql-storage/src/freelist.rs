//! SzRSQL 空闲页管理 — 对应 `SzRSQL实施进度.md` Phase 1.2。
//!
//! 设计要点：
//! - `next_page_id: AtomicU32` 单调递增，用于分配新页 ID
//! - `free_list: Mutex<Vec<u32>>` 栈式回收，LIFO 提高缓存局部性
//! - `allocate()`: 优先从 free_list 弹出，否则递增 next_page_id
//! - `free(page_id)`: 压入 free_list
//!
//! 线程安全：`AtomicU32` + `Mutex` 保证多线程并发安全。
//!
//! 契约：
//! - 调用方负责不重复 free 同一个 page_id（本模块不检测，避免 HashSet 开销）
//! - 调用方负责只 free 已分配的 page_id（< next_page_id）

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// =====================================================================
//  常量
// =====================================================================

/// 默认起始 page_id（page 0 保留给元数据页）
pub const FREELIST_DEFAULT_START: u32 = 1;

// =====================================================================
//  FreeListError
// =====================================================================

/// 空闲页管理错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FreeListError {
    #[error("page_id {0} overflow: exceeds u32::MAX")]
    PageIdOverflow(u64),
    #[error("invalid page_id: {page_id} (must be >= start_page_id {start})")]
    InvalidPageId { page_id: u32, start: u32 },
}

// =====================================================================
//  FreeList
// =====================================================================

/// 空闲页管理器
///
/// 线程安全：可跨线程并发 allocate/free。
pub struct FreeList {
    /// 下一个待分配的新 page_id（u64 计数器，避免 u32 溢出处理复杂度）
    next_page_id: AtomicU64,
    /// 起始 page_id（用于校验和统计）
    start_page_id: u32,
    /// 回收的 page_id 栈（LIFO）
    free_list: Mutex<Vec<u32>>,
}

impl FreeList {
    /// 创建默认 FreeList，起始 page_id = 1（page 0 保留给元数据）
    pub fn new() -> Self {
        Self::with_start(FREELIST_DEFAULT_START)
    }

    /// 创建指定起始 page_id 的 FreeList
    pub fn with_start(start_page_id: u32) -> Self {
        Self {
            next_page_id: AtomicU64::new(u64::from(start_page_id)),
            start_page_id,
            free_list: Mutex::new(Vec::new()),
        }
    }

    /// 分配一个 page_id
    ///
    /// 优先从 free_list 弹出回收的 ID，否则递增 next_page_id 计数器。
    /// 若 next_page_id 超过 u32::MAX，返回 `FreeListError::PageIdOverflow`。
    pub fn allocate(&self) -> Result<u32, FreeListError> {
        // 优先从 free_list 取
        {
            let mut free = self.free_list.lock().unwrap();
            if let Some(page_id) = free.pop() {
                return Ok(page_id);
            }
        }
        // free_list 为空，fetch_add 递增 u64 计数器（无 wraparound 风险）
        let current = self.next_page_id.fetch_add(1, Ordering::Relaxed);
        if current > u64::from(u32::MAX) {
            return Err(FreeListError::PageIdOverflow(current));
        }
        Ok(current as u32)
    }

    /// 回收一个 page_id，压入 free_list 供后续重用
    ///
    /// 注：调用方负责保证 page_id 合法且未被重复 free。
    pub fn free(&self, page_id: u32) {
        let mut free = self.free_list.lock().unwrap();
        free.push(page_id);
    }

    /// 批量分配 `n` 个 page_id
    ///
    /// 比逐个 allocate 更高效（减少锁竞争）。
    pub fn allocate_batch(&self, n: usize) -> Result<Vec<u32>, FreeListError> {
        let mut result = Vec::with_capacity(n);
        let mut free = self.free_list.lock().unwrap();
        // 先从 free_list 取
        while result.len() < n {
            if let Some(page_id) = free.pop() {
                result.push(page_id);
            } else {
                break;
            }
        }
        drop(free);
        // 剩余从计数器取（CAS 防止 wraparound）
        while result.len() < n {
            let allocated = self.allocate()?;
            result.push(allocated);
        }
        Ok(result)
    }

    /// 批量回收 page_ids
    pub fn free_batch(&self, page_ids: &[u32]) {
        let mut free = self.free_list.lock().unwrap();
        free.extend_from_slice(page_ids);
    }

    /// 当前已从计数器分配的总页数（不含回收的）
    ///
    /// = `next_page_id - start_page_id`
    pub fn total_allocated(&self) -> u64 {
        self.next_page_id
            .load(Ordering::Relaxed)
            .saturating_sub(u64::from(self.start_page_id))
    }

    /// 当前 free_list 中的回收页数
    pub fn free_count(&self) -> usize {
        self.free_list.lock().unwrap().len()
    }

    /// 当前实际占用页数 = total_allocated - free_count
    pub fn outstanding(&self) -> u64 {
        let total = self.total_allocated();
        total.saturating_sub(self.free_count() as u64)
    }

    /// 起始 page_id
    pub fn start_page_id(&self) -> u32 {
        self.start_page_id
    }

    /// 下一个将分配的新 page_id（不含 free_list）
    pub fn next_page_id(&self) -> u64 {
        self.next_page_id.load(Ordering::Relaxed)
    }
}

impl Default for FreeList {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{prop_assert, prop_assert_eq};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    // -----------------------------------------------------------------
    //  基础功能测试
    // -----------------------------------------------------------------

    #[test]
    fn new_defaults_start_at_1() {
        let fl = FreeList::new();
        assert_eq!(fl.start_page_id(), 1);
        assert_eq!(fl.next_page_id(), 1);
        assert_eq!(fl.total_allocated(), 0);
        assert_eq!(fl.free_count(), 0);
        assert_eq!(fl.outstanding(), 0);
    }

    #[test]
    fn with_start_custom_page_id() {
        let fl = FreeList::with_start(100);
        assert_eq!(fl.start_page_id(), 100);
        assert_eq!(fl.next_page_id(), 100);
        let p = fl.allocate().unwrap();
        assert_eq!(p, 100);
        assert_eq!(fl.next_page_id(), 101);
    }

    #[test]
    fn allocate_returns_sequential_ids() {
        let fl = FreeList::new();
        assert_eq!(fl.allocate().unwrap(), 1);
        assert_eq!(fl.allocate().unwrap(), 2);
        assert_eq!(fl.allocate().unwrap(), 3);
        assert_eq!(fl.total_allocated(), 3);
        assert_eq!(fl.free_count(), 0);
        assert_eq!(fl.outstanding(), 3);
    }

    #[test]
    fn free_then_allocate_reuses_id_lifo() {
        let fl = FreeList::new();
        let p1 = fl.allocate().unwrap();
        let p2 = fl.allocate().unwrap();
        let _p3 = fl.allocate().unwrap();
        // 释放 p2, p1
        fl.free(p2);
        fl.free(p1);
        assert_eq!(fl.free_count(), 2);
        assert_eq!(fl.outstanding(), 1); // p3 仍占用
                                         // LIFO: 先弹出 p1（后压入的）
        assert_eq!(fl.allocate().unwrap(), p1);
        assert_eq!(fl.allocate().unwrap(), p2);
        assert_eq!(fl.free_count(), 0);
        assert_eq!(fl.outstanding(), 3);
    }

    #[test]
    fn allocate_after_free_continues_sequential_when_empty() {
        let fl = FreeList::new();
        let p1 = fl.allocate().unwrap();
        fl.free(p1);
        // 从 free_list 取
        assert_eq!(fl.allocate().unwrap(), p1);
        // free_list 空，从计数器取
        assert_eq!(fl.allocate().unwrap(), 2);
    }

    #[test]
    fn total_allocated_does_not_decrement_on_free() {
        let fl = FreeList::new();
        fl.allocate().unwrap();
        fl.allocate().unwrap();
        fl.allocate().unwrap();
        assert_eq!(fl.total_allocated(), 3);
        fl.free(1);
        fl.free(2);
        assert_eq!(fl.total_allocated(), 3); // 不减少
        assert_eq!(fl.free_count(), 2);
        assert_eq!(fl.outstanding(), 1);
    }

    // -----------------------------------------------------------------
    //  唯一性测试
    // -----------------------------------------------------------------

    #[test]
    fn allocate_1000_ids_all_unique() {
        let fl = FreeList::new();
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let p = fl.allocate().unwrap();
            assert!(seen.insert(p), "duplicate page_id: {}", p);
        }
        assert_eq!(seen.len(), 1000);
    }

    #[test]
    fn allocate_free_allocate_cycle_no_duplicate() {
        let fl = FreeList::new();
        let mut held = HashSet::new();
        // 分配 100
        for _ in 0..100 {
            let p = fl.allocate().unwrap();
            assert!(held.insert(p), "duplicate in held: {}", p);
        }
        // 释放 50（从 held 移除）
        let to_free: Vec<u32> = held.iter().take(50).copied().collect();
        for p in to_free {
            held.remove(&p);
            fl.free(p);
        }
        // 再分配 50，应从 free_list 重用（不产生新 ID）
        for _ in 0..50 {
            let p = fl.allocate().unwrap();
            assert!(held.insert(p), "duplicate after reuse: {}", p);
        }
        assert_eq!(held.len(), 100);
        assert_eq!(fl.total_allocated(), 100); // 计数器只到了 100
        assert_eq!(fl.outstanding(), 100); // 100 占用
    }

    // -----------------------------------------------------------------
    //  批量操作测试
    // -----------------------------------------------------------------

    #[test]
    fn allocate_batch_returns_n_ids() {
        let fl = FreeList::new();
        let ids = fl.allocate_batch(100).unwrap();
        assert_eq!(ids.len(), 100);
        let set: HashSet<u32> = ids.iter().copied().collect();
        assert_eq!(set.len(), 100);
        assert_eq!(fl.total_allocated(), 100);
    }

    #[test]
    fn allocate_batch_mixed_free_list_and_counter() {
        let fl = FreeList::new();
        // 先释放一些到 free_list
        fl.free(5);
        fl.free(10);
        fl.free(15);
        assert_eq!(fl.free_count(), 3);
        // 批量分配 5 个：3 个从 free_list（LIFO: 15, 10, 5），2 个从计数器（1, 2）
        let ids = fl.allocate_batch(5).unwrap();
        assert_eq!(ids.len(), 5);
        assert_eq!(ids[0], 15); // LIFO
        assert_eq!(ids[1], 10);
        assert_eq!(ids[2], 5);
        assert_eq!(ids[3], 1); // 计数器
        assert_eq!(ids[4], 2);
        assert_eq!(fl.free_count(), 0);
        assert_eq!(fl.total_allocated(), 2);
    }

    #[test]
    fn free_batch_pushes_all() {
        let fl = FreeList::new();
        let ids: Vec<u32> = (1..=50).collect();
        fl.free_batch(&ids);
        assert_eq!(fl.free_count(), 50);
    }

    #[test]
    fn allocate_batch_zero_returns_empty() {
        let fl = FreeList::new();
        let ids = fl.allocate_batch(0).unwrap();
        assert!(ids.is_empty());
    }

    // -----------------------------------------------------------------
    //  边界值测试
    // -----------------------------------------------------------------

    #[test]
    fn allocate_at_u32_max_boundary() {
        // 起始接近 u32::MAX
        let fl = FreeList::with_start(u32::MAX - 2);
        assert_eq!(fl.allocate().unwrap(), u32::MAX - 2);
        assert_eq!(fl.allocate().unwrap(), u32::MAX - 1);
        assert_eq!(fl.allocate().unwrap(), u32::MAX);
        // 下一次应该溢出
        let result = fl.allocate();
        assert!(matches!(result, Err(FreeListError::PageIdOverflow(_))));
    }

    #[test]
    fn free_count_after_mixed_ops() {
        let fl = FreeList::new();
        // 先分配 100 个（契约：free 前必须 allocate）
        let mut allocated = Vec::new();
        for _ in 0..100 {
            allocated.push(fl.allocate().unwrap());
        }
        // 释放全部
        for p in allocated.drain(..) {
            fl.free(p);
        }
        assert_eq!(fl.free_count(), 100);
        assert_eq!(fl.outstanding(), 0);
        // 再分配 50 个
        for _ in 0..50 {
            fl.allocate().unwrap();
        }
        assert_eq!(fl.free_count(), 50);
        assert_eq!(fl.outstanding(), 50);
    }

    // -----------------------------------------------------------------
    //  并发测试
    // -----------------------------------------------------------------

    #[test]
    fn concurrent_allocate_no_duplicate_8_threads() {
        let fl = Arc::new(FreeList::new());
        let threads = 8;
        let per_thread = 125_000; // 总计 1,000,000
        let mut handles = Vec::new();
        for _ in 0..threads {
            let fl = Arc::clone(&fl);
            handles.push(thread::spawn(move || {
                let mut ids = Vec::with_capacity(per_thread);
                for _ in 0..per_thread {
                    ids.push(fl.allocate().unwrap());
                }
                ids
            }));
        }
        let mut all_ids = HashSet::new();
        for h in handles {
            let ids = h.join().unwrap();
            for id in ids {
                assert!(all_ids.insert(id), "duplicate page_id: {}", id);
            }
        }
        assert_eq!(all_ids.len(), threads * per_thread);
        assert_eq!(fl.total_allocated() as usize, threads * per_thread);
        assert_eq!(fl.free_count(), 0);
    }

    #[test]
    fn concurrent_allocate_free_1m_ops_balanced() {
        // 每个线程: 分配 N 个，释放 N 个，最终 free_list 非空，outstanding = 0
        let fl = Arc::new(FreeList::new());
        let threads = 8;
        let per_thread = 125_000; // 总计 1,000,000 allocate + 1,000,000 free
        let mut handles = Vec::new();
        for _ in 0..threads {
            let fl = Arc::clone(&fl);
            handles.push(thread::spawn(move || {
                let mut ids = Vec::with_capacity(per_thread);
                for _ in 0..per_thread {
                    ids.push(fl.allocate().unwrap());
                }
                // 全部释放
                for id in ids.drain(..) {
                    fl.free(id);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 所有页都释放了
        assert_eq!(fl.outstanding(), 0);
        assert_eq!(fl.free_count() as u64, fl.total_allocated());
    }

    #[test]
    fn concurrent_mixed_allocate_free_no_lost() {
        // 每个线程: 反复 allocate + free，跟踪净占用
        let fl = Arc::new(FreeList::new());
        let threads = 16;
        let per_thread = 62_500; // 总计 1,000,000 ops
        let mut handles = Vec::new();
        for _ in 0..threads {
            let fl = Arc::clone(&fl);
            handles.push(thread::spawn(move || {
                let mut outstanding: u64 = 0;
                for i in 0..per_thread {
                    let p = fl.allocate().unwrap();
                    if i % 2 == 1 {
                        fl.free(p);
                    } else {
                        outstanding += 1;
                    }
                }
                outstanding
            }));
        }
        let mut total_outstanding: u64 = 0;
        for h in handles {
            total_outstanding += h.join().unwrap();
        }
        // 每线程 per_thread/2 个未释放
        let expected_outstanding = (threads * per_thread / 2) as u64;
        assert_eq!(total_outstanding, expected_outstanding);
        assert_eq!(fl.outstanding(), expected_outstanding);
    }

    #[test]
    fn concurrent_allocate_batch_no_duplicate() {
        let fl = Arc::new(FreeList::new());
        let threads = 8;
        let batch_per_thread = 1000;
        let batch_size = 125; // 总计 8 * 1000 * 125 = 1,000,000
        let mut handles = Vec::new();
        for _ in 0..threads {
            let fl = Arc::clone(&fl);
            handles.push(thread::spawn(move || {
                let mut all = Vec::new();
                for _ in 0..batch_per_thread {
                    let ids = fl.allocate_batch(batch_size).unwrap();
                    all.extend(ids);
                }
                all
            }));
        }
        let mut seen = HashSet::new();
        for h in handles {
            let ids = h.join().unwrap();
            for id in ids {
                assert!(seen.insert(id), "duplicate: {}", id);
            }
        }
        assert_eq!(seen.len(), threads * batch_per_thread * batch_size);
    }

    // -----------------------------------------------------------------
    //  Proptest
    // -----------------------------------------------------------------

    proptest::proptest! {
        #[test]
        fn prop_allocate_free_cycle_no_duplicate(
            cycles in 1usize..=100,
            alloc_per_cycle in 1usize..=100,
        ) {
            let fl = FreeList::new();
            let mut held = HashSet::new();
            for _ in 0..cycles {
                // 分配
                for _ in 0..alloc_per_cycle {
                    let p = fl.allocate().unwrap();
                    prop_assert!(held.insert(p), "dup in held: {}", p);
                }
                // 释放一半（从 held 移除并 free）
                let half = held.len() / 2;
                let to_free: Vec<u32> = held.iter().take(half).copied().collect();
                for p in to_free {
                    held.remove(&p);
                    fl.free(p);
                }
            }
            // 最终: outstanding = held.len()
            prop_assert_eq!(fl.outstanding(), held.len() as u64);
        }

        #[test]
        fn prop_batch_allocate_count_matches(
            batch_size in 0usize..=500,
        ) {
            let fl = FreeList::new();
            let ids = fl.allocate_batch(batch_size).unwrap();
            prop_assert_eq!(ids.len(), batch_size);
            let set: HashSet<u32> = ids.iter().copied().collect();
            prop_assert_eq!(set.len(), batch_size, "batch has duplicates");
        }
    }
}
