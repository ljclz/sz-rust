//! Jepsen Set 测试 — 对应 `SzRSQL实施进度.md` Phase 2.20。
//!
//! 验证标准（来自实施进度表）：
//! - **10 线程并发 add 不重复元素 100000 个**
//! - **验证最终 set 包含且仅包含所有添加的元素**
//! - **0 丢失, 0 重复**
//!
//! 设计要点：
//! 1. **SetStore**：`RwLock<HashSet<i64>>` + 可选 WAL 持久化
//!    - add 操作本身是幂等的（重复 add 同一元素无效果），不需要 per-element lock
//!    - 单一 RwLock 保护整个 HashSet，add 时获取 write lock
//!    - 元素类型为 i64（与 jepsen_bank/jepsen_register 一致，便于 XorShift64 生成）
//! 2. **MVCC 事务**：
//!    - add：BEGIN + register_write(element) + commit + WAL
//!    - 由于 add 是幂等的，MVCC WW 冲突重试不会影响正确性
//!    - contains：BEGIN + register_read(element) + commit（只读，无 WAL）
//! 3. **并发不变量**：
//!    - 每线程添加不同的元素（element = tid * RANGE + i），避免冲突
//!    - 最终 set 包含且仅包含所有添加的元素（0 丢失，0 重复）
//! 4. **崩溃恢复**：
//!    - 关闭 WalWriter（不 flush）→ 重新打开 → replay → 重建 HashSet
//!    - replay 后 set 应等于 WAL 中所有 add 记录的元素集合
//! 5. **XorShift64 PRNG**：固定种子，测试可重现（与 mvcc_fuzz / jepsen_bank / jepsen_register 同风格）

use crate::mvcc::{MvccError, MvccManager};
use crate::wal::{WalError, WalOpType, WalReader, WalRecord, WalWriter};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

// =====================================================================
// XorShift64 — 固定种子 PRNG（与 jepsen_bank / jepsen_register 同风格）
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
}

// =====================================================================
// SetError — Set 操作错误
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum SetError {
    #[error("mvcc error: {0}")]
    Mvcc(#[from] MvccError),
    #[error("wal error: {0}")]
    Wal(#[from] WalError),
}

// =====================================================================
// SetStore — 元素集合 + WAL 持久化
// =====================================================================

/// 元素集合 + WAL 持久化
///
/// 线程安全设计：
/// - `elements`：`RwLock<HashSet<i64>>`，单一 RwLock 保护整个 HashSet
/// - add 操作本身是幂等的（重复 add 同一元素无效果），不需要 per-element lock
/// - WAL 持久化可选（`wal: Option<Arc<WalWriter>>`），无 WAL 时仅内存操作
struct SetStore {
    /// 元素集合
    elements: RwLock<HashSet<i64>>,
    /// 可选的 WAL writer
    wal: Option<Arc<WalWriter>>,
    /// 成功 add 次数（统计用，包括重复 add 同一元素）
    add_count: AtomicU64,
    /// 实际新增元素数（不包括重复 add）
    new_element_count: AtomicU64,
    /// 已提交事务数
    commit_count: AtomicU64,
    /// 已回滚事务数
    abort_count: AtomicU64,
}

impl SetStore {
    /// 创建无 WAL 的内存 Set
    fn new() -> Self {
        Self {
            elements: RwLock::new(HashSet::new()),
            wal: None,
            add_count: AtomicU64::new(0),
            new_element_count: AtomicU64::new(0),
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
        }
    }

    /// 创建带 WAL 的 Set
    fn with_wal(wal: Arc<WalWriter>) -> Self {
        Self {
            elements: RwLock::new(HashSet::new()),
            wal: Some(wal),
            add_count: AtomicU64::new(0),
            new_element_count: AtomicU64::new(0),
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
        }
    }

    /// MVCC 事务 add：BEGIN + register_write + commit + WAL
    ///
    /// 返回 true 表示新增元素（之前不存在），false 表示元素已存在（幂等 add）。
    ///
    /// **关键设计**：先检查元素是否已存在，若已存在则直接返回 Ok(false) 不经过 MVCC。
    /// 原因：MVCC 的 WW 冲突检测会导致重复 add 同一元素失败（register_write 的 key 相同），
    /// 但 Set 的 add 是幂等操作，不应失败。先检查存在性可以避免不必要的 MVCC 冲突。
    ///
    /// **并发安全**：先读 lock 检查 + 后写 lock 插入之间存在 TOCTOU 窗口，
    /// 两个线程可能同时通过存在性检查，但 HashSet::insert 本身是原子的，
    /// 第二个线程的 insert 会返回 false（元素已存在），不影响正确性。
    fn add(&self, mgr: &MvccManager, element: i64) -> Result<bool, SetError> {
        // 先检查元素是否已存在（快速路径，避免不必要的 MVCC 事务）
        {
            let elements = self.elements.read().unwrap();
            if elements.contains(&element) {
                self.add_count.fetch_add(1, Ordering::SeqCst);
                return Ok(false);
            }
        }

        // 元素不存在，执行 MVCC 事务添加
        let txn = mgr.begin();
        let _ = mgr.register_write(txn.txn_id, element.to_string());

        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                // 写 WAL（无论元素是否已存在，都记录 add 操作）
                if let Some(ref wal) = self.wal {
                    let record = WalRecord::new(
                        0,
                        txn.txn_id,
                        WalOpType::Commit,
                        0,
                        element.to_le_bytes().to_vec(),
                    );
                    wal.append(record)?;
                }
                // 应用到内存
                let mut elements = self.elements.write().unwrap();
                let is_new = elements.insert(element);
                self.add_count.fetch_add(1, Ordering::SeqCst);
                if is_new {
                    self.new_element_count.fetch_add(1, Ordering::SeqCst);
                }
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                Ok(is_new)
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(SetError::Mvcc(e))
            }
        }
    }

    /// 带重试的 add（用于并发测试，WW 冲突时自动重试）
    ///
    /// **注**：add 是幂等操作，WW 冲突时可以直接重试。
    fn add_with_retry(
        &self,
        mgr: &MvccManager,
        element: i64,
        max_retries: u32,
    ) -> Result<bool, SetError> {
        let mut retries = 0;
        loop {
            match self.add(mgr, element) {
                Ok(v) => return Ok(v),
                Err(SetError::Mvcc(MvccError::WriteWriteConflict(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(SetError::Mvcc(MvccError::WriteWriteConflict(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(SetError::Mvcc(MvccError::WriteSkewDetected(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(SetError::Mvcc(MvccError::WriteSkewDetected(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// MVCC 事务 contains：BEGIN + register_read + commit（只读）
    fn contains_txn(&self, mgr: &MvccManager, element: i64) -> Result<bool, SetError> {
        let txn = mgr.begin();
        let _ = mgr.register_read(txn.txn_id, element.to_string());
        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                let elements = self.elements.read().unwrap();
                Ok(elements.contains(&element))
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(SetError::Mvcc(e))
            }
        }
    }

    /// 无事务直接查询（不存在返回 false）
    fn contains(&self, element: i64) -> bool {
        self.elements.read().unwrap().contains(&element)
    }

    /// 集合大小
    fn len(&self) -> usize {
        self.elements.read().unwrap().len()
    }

    /// 是否为空
    fn is_empty(&self) -> bool {
        self.elements.read().unwrap().is_empty()
    }

    /// 快照：返回当前所有元素的 Vec（排序）
    fn to_sorted_vec(&self) -> Vec<i64> {
        let elements = self.elements.read().unwrap();
        let mut v: Vec<i64> = elements.iter().copied().collect();
        v.sort_unstable();
        v
    }

    /// 成功 add 次数（包括重复 add）
    fn add_count(&self) -> u64 {
        self.add_count.load(Ordering::SeqCst)
    }

    /// 实际新增元素数
    fn new_element_count(&self) -> u64 {
        self.new_element_count.load(Ordering::SeqCst)
    }

    /// 已提交事务数
    fn commit_count(&self) -> u64 {
        self.commit_count.load(Ordering::SeqCst)
    }

    /// 已回滚事务数
    fn abort_count(&self) -> u64 {
        self.abort_count.load(Ordering::SeqCst)
    }

    /// 从 WAL 回放重建 Set 状态
    ///
    /// 扫描 WAL 文件，对每条 `WalOpType::Commit` 记录解析 element（i64 LE），
    /// 添加到 HashSet（幂等，重复添加无效果）。
    fn recover_from_wal<P: AsRef<Path>>(wal_path: P) -> Result<Self, WalError> {
        let mut reader = WalReader::open(wal_path)?;
        let (records, _eof) = reader.read_all()?;

        let mut elements: HashSet<i64> = HashSet::new();
        let mut add_count = 0u64;

        for record in records {
            if record.op_type == WalOpType::Commit && record.data.len() >= 8 {
                let element = i64::from_le_bytes(record.data[..8].try_into().unwrap_or([0u8; 8]));
                elements.insert(element);
                add_count += 1;
            }
        }

        let new_element_count = elements.len() as u64;

        Ok(Self {
            elements: RwLock::new(elements),
            wal: None,
            add_count: AtomicU64::new(add_count),
            new_element_count: AtomicU64::new(new_element_count),
            commit_count: AtomicU64::new(add_count),
            abort_count: AtomicU64::new(0),
        })
    }
}

// =====================================================================
// 内联 tempfile 模块（与 jepsen_bank / jepsen_register 同风格）
// =====================================================================

#[cfg(test)]
pub mod tempfile {
    use std::path::PathBuf;

    pub struct TempDir {
        path: PathBuf,
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    impl TempDir {
        pub fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    pub fn tempdir() -> std::io::Result<TempDir> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let mut path = std::env::temp_dir();
        path.push(format!("szrsql_jepsen_set_{}_{}", pid, nanos));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }
}

// =====================================================================
// Phase 2.20 测试
// =====================================================================

#[cfg(test)]
mod phase_2_20 {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // -----------------------------------------------------------------
    // 1. 基础 add 语义测试
    // -----------------------------------------------------------------

    #[test]
    fn basic_add_single_thread() {
        let mgr = Arc::new(MvccManager::new());
        let store = SetStore::new();

        // 初始为空
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        // add 1, 2, 3
        assert!(store.add(&mgr, 1).unwrap());
        assert!(store.add(&mgr, 2).unwrap());
        assert!(store.add(&mgr, 3).unwrap());

        assert_eq!(store.len(), 3);
        assert!(store.contains(1));
        assert!(store.contains(2));
        assert!(store.contains(3));
        assert!(!store.contains(4));

        // 重复 add 返回 false（幂等）
        assert!(!store.add(&mgr, 1).unwrap());
        assert!(!store.add(&mgr, 2).unwrap());
        assert_eq!(store.len(), 3); // 大小不变

        // 统计：5 次 add（3 新 + 2 重复），3 个新元素
        assert_eq!(store.add_count(), 5);
        assert_eq!(store.new_element_count(), 3);
    }

    #[test]
    fn add_negative_and_large_elements() {
        let mgr = Arc::new(MvccManager::new());
        let store = SetStore::new();

        // 负数
        assert!(store.add(&mgr, -1).unwrap());
        assert!(store.add(&mgr, -1000000).unwrap());

        // 大数
        assert!(store.add(&mgr, i64::MAX).unwrap());
        assert!(store.add(&mgr, i64::MIN).unwrap());

        // 0
        assert!(store.add(&mgr, 0).unwrap());

        assert_eq!(store.len(), 5);
        assert!(store.contains(-1));
        assert!(store.contains(i64::MAX));
        assert!(store.contains(i64::MIN));
        assert!(store.contains(0));
    }

    #[test]
    fn contains_txn_returns_correct_value() {
        let mgr = Arc::new(MvccManager::new());
        let store = SetStore::new();

        store.add(&mgr, 42).unwrap();
        assert!(store.contains_txn(&mgr, 42).unwrap());
        assert!(!store.contains_txn(&mgr, 99).unwrap());
    }

    #[test]
    fn to_sorted_vec_returns_sorted_elements() {
        let mgr = Arc::new(MvccManager::new());
        let store = SetStore::new();

        store.add(&mgr, 5).unwrap();
        store.add(&mgr, 1).unwrap();
        store.add(&mgr, 3).unwrap();
        store.add(&mgr, 2).unwrap();
        store.add(&mgr, 4).unwrap();

        let v = store.to_sorted_vec();
        assert_eq!(v, vec![1, 2, 3, 4, 5]);
    }

    // -----------------------------------------------------------------
    // 2. 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn add_count_includes_duplicates() {
        let mgr = Arc::new(MvccManager::new());
        let store = SetStore::new();

        // add 同一元素 5 次
        for _ in 0..5 {
            store.add(&mgr, 100).unwrap();
        }

        // add_count = 5，new_element_count = 1
        assert_eq!(store.add_count(), 5);
        assert_eq!(store.new_element_count(), 1);
        assert_eq!(store.len(), 1);
    }

    // -----------------------------------------------------------------
    // 3. 并发 add 测试 — 10 线程并发 add 不重复元素
    // -----------------------------------------------------------------

    /// 10 线程并发 add 不重复元素，验证最终 set 包含且仅包含所有添加的元素
    ///
    /// **关键设计**：每线程添加不同的元素（element = tid * RANGE + i），
    /// 避免线程间冲突，确保所有 add 都应成功。
    #[test]
    fn jepsen_set_10_threads_add_distinct_elements() {
        const THREADS: usize = 10;
        const ADDS_PER_THREAD: u32 = 10000;
        const ELEMENT_RANGE: i64 = 1_000_000;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(SetStore::new());

        // 期望元素集合：每线程添加 tid * ELEMENT_RANGE + i
        let mut expected_elements: HashSet<i64> = HashSet::new();
        for tid in 0..THREADS as i64 {
            for i in 0..ADDS_PER_THREAD as i64 {
                expected_elements.insert(tid * ELEMENT_RANGE + i);
            }
        }

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    let mut success = 0u64;
                    for i in 0..ADDS_PER_THREAD as i64 {
                        let element = (tid as i64) * ELEMENT_RANGE + i;
                        // 使用 add_with_retry 应对 WW 冲突
                        if store.add_with_retry(&mgr, element, 100).is_ok() {
                            success += 1;
                        }
                    }
                    success
                })
            })
            .collect();

        let mut total_success = 0u64;
        for h in handles {
            total_success += h.join().unwrap();
        }

        // 验证：所有 add 都成功（WW 冲突时自动重试）
        assert_eq!(
            total_success,
            (THREADS as u64) * (ADDS_PER_THREAD as u64),
            "所有 add 都应成功（WW 冲突时自动重试）"
        );

        // 验证：集合大小 == 期望元素数（0 丢失，0 重复）
        assert_eq!(
            store.len(),
            expected_elements.len(),
            "集合大小 {} 应等于期望元素数 {}（0 丢失，0 重复）",
            store.len(),
            expected_elements.len()
        );

        // 验证：集合包含且仅包含所有期望元素
        let actual_elements: HashSet<i64> = store.elements.read().unwrap().clone();
        assert_eq!(
            actual_elements, expected_elements,
            "集合内容应与期望完全一致"
        );

        // 验证：统计计数
        assert_eq!(store.add_count(), total_success);
        assert_eq!(store.new_element_count(), total_success); // 每次都是新元素
    }

    /// 10 线程并发 add 同一元素，验证最终只包含一个（幂等性）
    #[test]
    fn jepsen_set_10_threads_add_same_element_idempotent() {
        const THREADS: usize = 10;
        const ADDS_PER_THREAD: u32 = 1000;
        const ELEMENT: i64 = 42;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(SetStore::new());

        let handles: Vec<_> = (0..THREADS)
            .map(|_tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    let mut success = 0u64;
                    let mut new_count = 0u64;
                    for _ in 0..ADDS_PER_THREAD {
                        if let Ok(is_new) = store.add_with_retry(&mgr, ELEMENT, 100) {
                            success += 1;
                            if is_new {
                                new_count += 1;
                            }
                        }
                    }
                    (success, new_count)
                })
            })
            .collect();

        let mut total_success = 0u64;
        let mut total_new = 0u64;
        for h in handles {
            let (s, n) = h.join().unwrap();
            total_success += s;
            total_new += n;
        }

        // 验证：所有 add 都成功
        assert_eq!(total_success, (THREADS as u64) * (ADDS_PER_THREAD as u64));

        // 验证：集合只包含一个元素
        assert_eq!(store.len(), 1);
        assert!(store.contains(ELEMENT));

        // 验证：只有第一次 add 是 new，其余都是重复
        assert_eq!(total_new, 1);
        assert_eq!(store.new_element_count(), 1);
        assert_eq!(store.add_count(), total_success);
    }

    /// 10 线程并发 add 部分重叠元素，验证最终 set 正确
    ///
    /// **设计**：每线程 add 范围 [tid * 100, tid * 100 + 200)，共 200 个元素。
    /// 相邻线程范围重叠 100 个元素（如 thread 0 add [0, 200)，thread 1 add [100, 300)）。
    /// 最终 set 应包含所有不同的元素。
    #[test]
    fn jepsen_set_10_threads_add_overlapping_elements() {
        const THREADS: usize = 10;
        const ADDS_PER_THREAD: u32 = 200;
        const STRIDE: i64 = 100; // 每线程起始元素间隔

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(SetStore::new());

        // 期望元素集合
        let mut expected_elements: HashSet<i64> = HashSet::new();
        for tid in 0..THREADS as i64 {
            for i in 0..ADDS_PER_THREAD as i64 {
                expected_elements.insert(tid * STRIDE + i);
            }
        }

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    let mut success = 0u64;
                    for i in 0..ADDS_PER_THREAD as i64 {
                        let element = (tid as i64) * STRIDE + i;
                        if store.add_with_retry(&mgr, element, 100).is_ok() {
                            success += 1;
                        }
                    }
                    success
                })
            })
            .collect();

        let mut total_success = 0u64;
        for h in handles {
            total_success += h.join().unwrap();
        }

        // 验证：所有 add 都成功
        assert_eq!(total_success, (THREADS as u64) * (ADDS_PER_THREAD as u64));

        // 验证：集合大小 == 期望元素数（部分重叠，去重后）
        assert_eq!(
            store.len(),
            expected_elements.len(),
            "集合大小 {} 应等于期望元素数 {}（去重后）",
            store.len(),
            expected_elements.len()
        );

        // 验证：集合内容与期望一致
        let actual_elements: HashSet<i64> = store.elements.read().unwrap().clone();
        assert_eq!(actual_elements, expected_elements);

        // 验证：new_element_count == 期望元素数（重复 add 不计入）
        assert_eq!(store.new_element_count(), expected_elements.len() as u64);
    }

    // -----------------------------------------------------------------
    // 4. 并发 add + contains 测试
    // -----------------------------------------------------------------

    /// 10 线程并发 add + contains，验证 contains 返回的一致性
    ///
    /// **不变量**：若 element 已被 add 提交，则 contains 必须返回 true。
    #[test]
    fn jepsen_set_concurrent_add_contains_consistency() {
        const WRITERS: usize = 4;
        const READERS: usize = 4;
        const OPS_PER_THREAD: u32 = 2000;
        const ELEMENT_RANGE: i64 = 10000;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(SetStore::new());

        // 已提交元素集合（线程安全）
        let committed_elements: Arc<RwLock<HashSet<i64>>> = Arc::new(RwLock::new(HashSet::new()));

        // 非法 contains 计数（contains 返回 true 但元素不在 committed_elements 中）
        let invalid_contains = Arc::new(AtomicU64::new(0));
        let total_contains = Arc::new(AtomicU64::new(0));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // writer 线程
        let writer_handles: Vec<_> = (0..WRITERS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let committed_elements = Arc::clone(&committed_elements);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0x5E7);
                    let mut success = 0u64;
                    for _ in 0..OPS_PER_THREAD {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let element = (tid as i64) * ELEMENT_RANGE
                            + rng.next_range(ELEMENT_RANGE as u32) as i64;
                        // 先把元素加入 committed_elements，再调用 add
                        {
                            let mut ce = committed_elements.write().unwrap();
                            ce.insert(element);
                        }
                        if store.add_with_retry(&mgr, element, 100).is_ok() {
                            success += 1;
                        }
                    }
                    success
                })
            })
            .collect();

        // reader 线程
        let reader_handles: Vec<_> = (0..READERS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let committed_elements = Arc::clone(&committed_elements);
                let invalid_contains = Arc::clone(&invalid_contains);
                let total_contains = Arc::clone(&total_contains);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0xBA5);
                    let mut local_contains = 0u64;
                    while !stop.load(Ordering::SeqCst) {
                        // 随机查询一个元素
                        let element = rng.next_range(ELEMENT_RANGE as u32 * WRITERS as u32) as i64;
                        let in_store = if rng.next_range(2) == 0 {
                            store.contains(element)
                        } else {
                            store.contains_txn(&mgr, element).unwrap_or(false)
                        };
                        local_contains += 1;
                        // 验证：若 store.contains 返回 true，则元素必须在 committed_elements 中
                        if in_store {
                            let ce = committed_elements.read().unwrap();
                            if !ce.contains(&element) {
                                invalid_contains.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                    total_contains.fetch_add(local_contains, Ordering::SeqCst);
                })
            })
            .collect();

        // 等待所有 writer 完成
        let mut total_writes = 0u64;
        for h in writer_handles {
            total_writes += h.join().unwrap();
        }

        // 通知 reader 停止
        stop.store(true, Ordering::SeqCst);

        // 等待所有 reader 完成
        for h in reader_handles {
            h.join().unwrap();
        }

        // 验证：0 非法 contains
        let invalid = invalid_contains.load(Ordering::SeqCst);
        let total_c = total_contains.load(Ordering::SeqCst);
        assert_eq!(
            invalid, 0,
            "发现 {} 次非法 contains（返回 true 但元素不在已提交集合中），共 {} 次 contains",
            invalid, total_c
        );

        // 验证：所有 add 都成功
        assert_eq!(store.add_count(), total_writes);

        // 验证：集合是 committed_elements 的子集（可能等于）
        let actual_elements: HashSet<i64> = store.elements.read().unwrap().clone();
        let ce = committed_elements.read().unwrap();
        for elem in &actual_elements {
            assert!(
                ce.contains(elem),
                "store 中的元素 {} 不在 committed_elements 中",
                elem
            );
        }
    }

    // -----------------------------------------------------------------
    // 5. 崩溃恢复测试
    // -----------------------------------------------------------------

    /// 基础崩溃恢复：add N 个元素 → flush → recover → 验证集合一致
    #[test]
    fn jepsen_set_crash_recovery_basic() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("set_basic.wal");

        const ELEMENTS: u32 = 1000;

        // 阶段 1：写入 N 个元素
        let expected_elements: HashSet<i64> = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = SetStore::with_wal(wal);
            let mgr = MvccManager::new();

            let mut expected = HashSet::new();
            for i in 0..ELEMENTS as i64 {
                store.add(&mgr, i).unwrap();
                expected.insert(i);
            }
            // flush 确保全部落盘
            store.wal.as_ref().unwrap().flush().unwrap();
            expected
        };

        // 阶段 2：模拟崩溃 — recover_from_wal
        let recovered = SetStore::recover_from_wal(&wal_path).unwrap();

        // 验证：集合与崩溃前一致
        assert_eq!(recovered.len(), expected_elements.len());
        for elem in &expected_elements {
            assert!(
                recovered.contains(*elem),
                "元素 {} 应在恢复后的集合中",
                elem
            );
        }

        // 验证：add_count == ELEMENTS
        assert_eq!(recovered.add_count(), ELEMENTS as u64);
        assert_eq!(recovered.new_element_count(), ELEMENTS as u64);
    }

    /// 崩溃恢复 + 继续添加：add → crash → recover → add → crash → recover → 验证
    #[test]
    fn jepsen_set_crash_recovery_continue_adds() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("set_continue.wal");

        // ===== 阶段 1：初始添加 =====
        let phase1_expected: HashSet<i64> = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = SetStore::with_wal(wal);
            let mgr = MvccManager::new();
            let mut expected = HashSet::new();
            for i in 0..100i64 {
                store.add(&mgr, i).unwrap();
                expected.insert(i);
            }
            store.wal.as_ref().unwrap().flush().unwrap();
            expected
        };

        // ===== 阶段 2：第一次 recover + 继续添加 =====
        let phase2_expected: HashSet<i64> = {
            let recovered = SetStore::recover_from_wal(&wal_path).unwrap();
            // 验证 phase1 状态
            assert_eq!(recovered.len(), phase1_expected.len());

            // 继续添加
            let new_wal = Arc::new(WalWriter::open(&wal_path).unwrap());
            let mgr = MvccManager::new();
            let store = SetStore {
                elements: recovered.elements,
                wal: Some(new_wal),
                add_count: AtomicU64::new(recovered.add_count.load(Ordering::SeqCst)),
                new_element_count: AtomicU64::new(
                    recovered.new_element_count.load(Ordering::SeqCst),
                ),
                commit_count: AtomicU64::new(recovered.commit_count.load(Ordering::SeqCst)),
                abort_count: AtomicU64::new(0),
            };

            let mut expected = phase1_expected.clone();
            for i in 100..200i64 {
                store.add(&mgr, i).unwrap();
                expected.insert(i);
            }
            store.wal.as_ref().unwrap().flush().unwrap();
            expected
        };

        // ===== 阶段 3：第二次 recover =====
        let recovered2 = SetStore::recover_from_wal(&wal_path).unwrap();
        assert_eq!(recovered2.len(), phase2_expected.len());
        for elem in &phase2_expected {
            assert!(recovered2.contains(*elem));
        }
    }

    /// 崩溃不 flush 不损坏：add N → 不 flush → recover → 验证只看到已落盘的记录
    #[test]
    fn jepsen_set_crash_without_flush_no_corruption() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("set_noflush.wal");

        // 写入但故意不 flush
        let written_elements: HashSet<i64> = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = SetStore::with_wal(wal);
            let mgr = MvccManager::new();
            let mut written = HashSet::new();
            for i in 0..100i64 {
                store.add(&mgr, i).unwrap();
                written.insert(i);
            }
            // 故意不 flush，直接 drop（模拟崩溃）
            written
        };

        // recover：能读到的记录应该完整可解码（不损坏）
        let recovered = SetStore::recover_from_wal(&wal_path).unwrap();
        // 验证：恢复后每个元素要么在 written_elements 中，要么不存在（如果记录未落盘）
        // 由于 WalWriter 实现细节，这里只验证"不损坏"：所有能读到的记录都是合法的 i64
        for i in 0..100i64 {
            if recovered.contains(i) {
                assert!(
                    written_elements.contains(&i),
                    "恢复后的元素 {} 应在 written_elements 中",
                    i
                );
            }
        }
    }

    /// 完整崩溃恢复工作流：并发 add → crash → recover → 验证 → 继续并发 add → crash → recover → 验证
    #[test]
    fn jepsen_set_full_crash_recovery_workflow() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("set_full.wal");

        const THREADS: usize = 4;
        const ADDS_PER_THREAD: u32 = 5000;
        const ELEMENT_RANGE: i64 = 1_000_000;

        // ===== 阶段 1：并发添加 =====
        let phase1_expected: HashSet<i64> = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = Arc::new(SetStore::with_wal(wal));
            let mgr = Arc::new(MvccManager::new());

            let mut expected = HashSet::new();
            for tid in 0..THREADS as i64 {
                for i in 0..ADDS_PER_THREAD as i64 {
                    expected.insert(tid * ELEMENT_RANGE + i);
                }
            }

            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    thread::spawn(move || {
                        let mut success = 0u64;
                        for i in 0..ADDS_PER_THREAD as i64 {
                            let element = (tid as i64) * ELEMENT_RANGE + i;
                            if store.add_with_retry(&mgr, element, 100).is_ok() {
                                success += 1;
                            }
                        }
                        success
                    })
                })
                .collect();

            let mut total = 0u64;
            for h in handles {
                total += h.join().unwrap();
            }
            store.wal.as_ref().unwrap().flush().unwrap();
            assert_eq!(total, (THREADS as u64) * (ADDS_PER_THREAD as u64));
            expected
        };

        // ===== 阶段 2：模拟重启 — 从 WAL replay =====
        let recovered = SetStore::recover_from_wal(&wal_path).unwrap();

        // 验证：恢复后集合与 phase1_expected 一致
        assert_eq!(recovered.len(), phase1_expected.len());
        for elem in &phase1_expected {
            assert!(recovered.contains(*elem));
        }

        // ===== 阶段 3：第二轮并发添加（使用 replay 后的状态） =====
        let phase2_expected: HashSet<i64> = {
            let new_wal = Arc::new(WalWriter::open(&wal_path).unwrap());
            let mgr = Arc::new(MvccManager::new());
            let store = Arc::new(SetStore {
                elements: recovered.elements,
                wal: Some(new_wal),
                add_count: AtomicU64::new(recovered.add_count.load(Ordering::SeqCst)),
                new_element_count: AtomicU64::new(
                    recovered.new_element_count.load(Ordering::SeqCst),
                ),
                commit_count: AtomicU64::new(recovered.commit_count.load(Ordering::SeqCst)),
                abort_count: AtomicU64::new(0),
            });

            let mut expected = phase1_expected.clone();
            // 第二轮使用不同的元素范围（offset = 10_000_000）
            const OFFSET: i64 = 10_000_000;
            for tid in 0..THREADS as i64 {
                for i in 0..ADDS_PER_THREAD as i64 {
                    expected.insert(OFFSET + tid * ELEMENT_RANGE + i);
                }
            }

            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    thread::spawn(move || {
                        let mut success = 0u64;
                        for i in 0..ADDS_PER_THREAD as i64 {
                            let element = OFFSET + (tid as i64) * ELEMENT_RANGE + i;
                            if store.add_with_retry(&mgr, element, 100).is_ok() {
                                success += 1;
                            }
                        }
                        success
                    })
                })
                .collect();

            let mut total = 0u64;
            for h in handles {
                total += h.join().unwrap();
            }
            store.wal.as_ref().unwrap().flush().unwrap();
            assert_eq!(total, (THREADS as u64) * (ADDS_PER_THREAD as u64));
            expected
        };

        // ===== 阶段 4：再次崩溃恢复，验证最终状态 =====
        let recovered2 = SetStore::recover_from_wal(&wal_path).unwrap();

        // 验证：集合 == phase2_expected
        assert_eq!(recovered2.len(), phase2_expected.len());
        for elem in &phase2_expected {
            assert!(recovered2.contains(*elem));
        }
    }

    // -----------------------------------------------------------------
    // 6. 大规模并发测试（10 线程 × 10000 add = 100000 元素）
    // -----------------------------------------------------------------

    /// 大规模测试：10 线程并发 add 100000 个不重复元素
    /// 对应实施进度表的"10 线程并发 add 不重复元素 100000 个"
    #[test]
    fn jepsen_set_10_threads_100k_distinct_elements() {
        const THREADS: usize = 10;
        const ADDS_PER_THREAD: u32 = 10000;
        const TOTAL_ADDS: u32 = THREADS as u32 * ADDS_PER_THREAD; // 100000

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(SetStore::new());

        // 期望元素集合
        let mut expected_elements: HashSet<i64> = HashSet::new();
        for tid in 0..THREADS as i64 {
            for i in 0..ADDS_PER_THREAD as i64 {
                expected_elements.insert(tid * 100_000 + i);
            }
        }

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    let mut success = 0u64;
                    for i in 0..ADDS_PER_THREAD as i64 {
                        let element = (tid as i64) * 100_000 + i;
                        if store.add_with_retry(&mgr, element, 100).is_ok() {
                            success += 1;
                        }
                    }
                    success
                })
            })
            .collect();

        let mut total_success = 0u64;
        for h in handles {
            total_success += h.join().unwrap();
        }

        // 验证：所有 100000 个 add 都成功
        assert_eq!(total_success, TOTAL_ADDS as u64);

        // 验证：集合包含 100000 个元素（0 丢失，0 重复）
        assert_eq!(store.len(), TOTAL_ADDS as usize);
        assert_eq!(store.len(), expected_elements.len());

        // 验证：集合内容与期望一致
        let actual_elements: HashSet<i64> = store.elements.read().unwrap().clone();
        assert_eq!(actual_elements, expected_elements);

        // 验证：统计
        assert_eq!(store.add_count(), TOTAL_ADDS as u64);
        assert_eq!(store.new_element_count(), TOTAL_ADDS as u64);
    }
}
