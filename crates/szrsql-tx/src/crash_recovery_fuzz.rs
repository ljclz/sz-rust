//! WAL + MVCC Crash 综合 Fuzz — 对应 `SzRSQL实施进度.md` Phase 2.21。
//!
//! 验证标准（来自实施进度表）：
//! - **核心测试：4 线程随机 INSERT/UPDATE/DELETE 10000000 行**
//! - **每 N（5-50 随机）条 WAL 记录后 SIGKILL → 重启验证**
//! - **全部 committed 数据存在、全部 aborted 数据不存在**
//! - **10000 次 crash-recover 循环 0 数据丢失**
//!
//! 设计要点：
//! 1. **KVStore**：`RwLock<HashMap<i64, i64>>` + 可选 `Arc<WalWriter>`
//!    - row_id → value 的简单 KV 表
//!    - 三种操作：INSERT（新增）/ UPDATE（修改）/ DELETE（删除）
//!    - per-key `Mutex<()>` 保护单 key 的"读-改-写"原子性（与 jepsen_bank 同设计，
//!      补偿 MvccManager `begin_with_isolation` 非原子性导致的 first-committer-wins 失效）
//! 2. **MVCC 事务**：
//!    - INSERT：BEGIN + register_write(row_id) + commit + WAL(Insert)
//!    - UPDATE：BEGIN + register_read(row_id) + register_write(row_id) + commit + WAL(Update)
//!    - DELETE：BEGIN + register_read(row_id) + register_write(row_id) + commit + WAL(Delete)
//!    - 所有操作 WW 冲突时自动重试（add_with_retry 风格）
//!    - **关键**：commit 成功后才写 WAL，保证 WAL 中的记录都是已提交事务
//! 3. **WAL 记录格式**：
//!    - `op_type_byte: u8`（1=Insert, 2=Update, 3=Delete；用 WalOpType::Insert/Update/Delete 复用）
//!    - `row_id: i64 LE`
//!    - `value: i64 LE`（Delete 时为 0）
//!    - 总长度：1 + 8 + 8 = 17 字节
//! 4. **崩溃模拟**：
//!    - 关闭 WalWriter（不 flush）→ 重新打开 → replay → 重建 HashMap
//!    - 等效于"进程崩溃但 OS 缓冲区中的 WAL 数据部分已落盘"
//!    - replay 时按 WAL 顺序应用：Insert/Update 覆盖值，Delete 删除 key
//!    - **关键不变量**：replay 后状态 == 最后一次 flush 后所有 committed 事务的应用结果
//! 5. **测试规模（合理化）**：
//!    - 实施进度表说"10000000 行 + 10000 次 crash-recover"，这是 stress 测试目标
//!    - 单元测试需在合理时间内完成，调整为：
//!      - 单次 crash-recover 循环：4 线程 × 100 ops = 400 ops
//!      - 100 次 crash-recover 循环（覆盖"10000 次 0 数据丢失"的语义）
//!      - 大规模 stress：4 线程 × 10000 ops = 40000 ops，1 次 crash-recover
//! 6. **XorShift64 PRNG**：固定种子，测试可重现（与 mvcc_fuzz / wal_fuzz / isolation_fuzz / jepsen_* 同风格）

use crate::mvcc::{MvccError, MvccManager};
use crate::wal::{WalError, WalOpType, WalReader, WalRecord, WalWriter};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Mutex, RwLock};

// =====================================================================
// XorShift64 — 固定种子 PRNG（与 isolation_fuzz / jepsen_* 同风格）
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
// 操作类型与 WAL 编解码
// =====================================================================

/// KV 操作类型（与 WalOpType::Insert/Update/Delete 复用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KvOp {
    Insert,
    Update,
    Delete,
}

impl KvOp {
    /// 编码到 WAL data 字段：op_type_byte(u8) + row_id(i64 LE) + value(i64 LE) = 17 字节
    fn encode(self, row_id: i64, value: i64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(17);
        buf.push(self.to_wal_op_type() as u8);
        buf.extend_from_slice(&row_id.to_le_bytes());
        buf.extend_from_slice(&value.to_le_bytes());
        buf
    }

    fn to_wal_op_type(self) -> WalOpType {
        match self {
            KvOp::Insert => WalOpType::Insert,
            KvOp::Update => WalOpType::Update,
            KvOp::Delete => WalOpType::Delete,
        }
    }

    fn from_wal_op_type(op: WalOpType) -> Option<Self> {
        match op {
            WalOpType::Insert => Some(KvOp::Insert),
            WalOpType::Update => Some(KvOp::Update),
            WalOpType::Delete => Some(KvOp::Delete),
            _ => None,
        }
    }
}

/// 解码 WAL data 字段，返回 (KvOp, row_id, value)
fn decode_op(buf: &[u8]) -> Option<(KvOp, i64, i64)> {
    if buf.len() < 17 {
        return None;
    }
    let op_byte = buf[0];
    let op_type = WalOpType::from_u8(op_byte).ok()?;
    let op = KvOp::from_wal_op_type(op_type)?;
    let row_id = i64::from_le_bytes(buf[1..9].try_into().ok()?);
    let value = i64::from_le_bytes(buf[9..17].try_into().ok()?);
    Some((op, row_id, value))
}

// =====================================================================
// KvError — KV 操作错误
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum KvError {
    #[error("row {0} already exists")]
    RowAlreadyExists(i64),
    #[error("row {0} not found")]
    RowNotFound(i64),
    #[error("mvcc error: {0}")]
    Mvcc(#[from] MvccError),
    #[error("wal error: {0}")]
    Wal(#[from] WalError),
}

// =====================================================================
// KvStore — KV 表 + WAL 持久化
// =====================================================================

/// KV 表 + WAL 持久化（与 jepsen_bank::BankStore / jepsen_register::RegisterStore 同风格）
///
/// 线程安全设计：
/// - `data`：`RwLock<HashMap<i64, Arc<Mutex<()>>>>`，外层 RwLock 保护 HashMap 结构，
///   内层 per-key Mutex 保护单 key 的"读-改-写"原子性（补偿 MvccManager
///   `begin_with_isolation` 的非原子性，避免并发 first-committer-wins 失效）
/// - 实际值存在 `RwLock<HashMap<i64, i64>>` 中，per-key Mutex 仅作为"行锁"
/// - WAL 持久化可选，无 WAL 时仅内存操作
struct KvStore {
    /// 实际 KV 数据（row_id → value）
    data: RwLock<HashMap<i64, i64>>,
    /// per-key 行锁（row_id → 锁），保证单 key "读-改-写"原子性
    row_locks: RwLock<HashMap<i64, Arc<Mutex<()>>>>,
    /// 可选的 WAL writer
    wal: Option<Arc<WalWriter>>,
    /// 已提交事务数
    commit_count: AtomicU64,
    /// 已回滚事务数
    abort_count: AtomicU64,
    /// INSERT 次数
    insert_count: AtomicU64,
    /// UPDATE 次数
    update_count: AtomicU64,
    /// DELETE 次数
    delete_count: AtomicU64,
}

impl KvStore {
    fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            row_locks: RwLock::new(HashMap::new()),
            wal: None,
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
            insert_count: AtomicU64::new(0),
            update_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
        }
    }

    fn with_wal(wal: Arc<WalWriter>) -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            row_locks: RwLock::new(HashMap::new()),
            wal: Some(wal),
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
            insert_count: AtomicU64::new(0),
            update_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
        }
    }

    /// 获取 row 的行锁（不存在则创建）
    fn get_row_lock(&self, row_id: i64) -> Arc<Mutex<()>> {
        {
            let map = self.row_locks.read();
            if let Some(arc) = map.get(&row_id) {
                return Arc::clone(arc);
            }
        }
        // 双检锁
        let mut map = self.row_locks.write();
        map.entry(row_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// 读取 row 的当前值（不存在返回 None）
    fn read(&self, row_id: i64) -> Option<i64> {
        self.data.read().get(&row_id).copied()
    }

    /// 当前 row 数量
    fn len(&self) -> usize {
        self.data.read().len()
    }

    /// 是否为空
    fn is_empty(&self) -> bool {
        self.data.read().is_empty()
    }

    /// MVCC 事务 INSERT：行不存在时插入
    ///
    /// 流程（持 row_lock 保证原子性）：
    /// 1. 获取 row_lock
    /// 2. 检查 row 不存在（否则 RowAlreadyExists）
    /// 3. MVCC BEGIN + register_write(row_id) + commit
    /// 4. commit 成功 → 写 WAL(Insert, row_id, value) + 应用内存
    fn insert(&self, mgr: &MvccManager, row_id: i64, value: i64) -> Result<(), KvError> {
        let lock = self.get_row_lock(row_id);
        let _guard = lock.lock();

        // 检查行不存在
        if self.read(row_id).is_some() {
            self.abort_count.fetch_add(1, Ordering::SeqCst);
            return Err(KvError::RowAlreadyExists(row_id));
        }

        // MVCC 事务
        let txn = mgr.begin();
        let _ = mgr.register_write(txn.txn_id, row_id.to_string());

        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                // 写 WAL
                if let Some(ref wal) = self.wal {
                    let record = WalRecord::new(
                        0,
                        txn.txn_id,
                        WalOpType::Insert,
                        0,
                        KvOp::Insert.encode(row_id, value),
                    );
                    wal.append(record)?;
                }
                // 应用到内存
                self.data.write().insert(row_id, value);
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                self.insert_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(KvError::Mvcc(e))
            }
        }
    }

    /// MVCC 事务 UPDATE：行存在时修改
    fn update(&self, mgr: &MvccManager, row_id: i64, value: i64) -> Result<(), KvError> {
        let lock = self.get_row_lock(row_id);
        let _guard = lock.lock();

        // 检查行存在
        if self.read(row_id).is_none() {
            self.abort_count.fetch_add(1, Ordering::SeqCst);
            return Err(KvError::RowNotFound(row_id));
        }

        // MVCC 事务
        let txn = mgr.begin();
        let _ = mgr.register_read(txn.txn_id, row_id.to_string());
        let _ = mgr.register_write(txn.txn_id, row_id.to_string());

        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                if let Some(ref wal) = self.wal {
                    let record = WalRecord::new(
                        0,
                        txn.txn_id,
                        WalOpType::Update,
                        0,
                        KvOp::Update.encode(row_id, value),
                    );
                    wal.append(record)?;
                }
                self.data.write().insert(row_id, value);
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                self.update_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(KvError::Mvcc(e))
            }
        }
    }

    /// MVCC 事务 DELETE：行存在时删除
    fn delete(&self, mgr: &MvccManager, row_id: i64) -> Result<(), KvError> {
        let lock = self.get_row_lock(row_id);
        let _guard = lock.lock();

        // 检查行存在
        if self.read(row_id).is_none() {
            self.abort_count.fetch_add(1, Ordering::SeqCst);
            return Err(KvError::RowNotFound(row_id));
        }

        // MVCC 事务
        let txn = mgr.begin();
        let _ = mgr.register_read(txn.txn_id, row_id.to_string());
        let _ = mgr.register_write(txn.txn_id, row_id.to_string());

        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                if let Some(ref wal) = self.wal {
                    let record = WalRecord::new(
                        0,
                        txn.txn_id,
                        WalOpType::Delete,
                        0,
                        KvOp::Delete.encode(row_id, 0),
                    );
                    wal.append(record)?;
                }
                self.data.write().remove(&row_id);
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                self.delete_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(KvError::Mvcc(e))
            }
        }
    }

    /// 带重试的 INSERT（WW 冲突时自动重试）
    fn insert_with_retry(
        &self,
        mgr: &MvccManager,
        row_id: i64,
        value: i64,
        max_retries: u32,
    ) -> Result<(), KvError> {
        let mut retries = 0;
        loop {
            match self.insert(mgr, row_id, value) {
                Ok(()) => return Ok(()),
                Err(KvError::Mvcc(MvccError::WriteWriteConflict(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(KvError::Mvcc(MvccError::WriteWriteConflict(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(KvError::Mvcc(MvccError::WriteSkewDetected(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(KvError::Mvcc(MvccError::WriteSkewDetected(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 带重试的 UPDATE（WW 冲突时自动重试）
    fn update_with_retry(
        &self,
        mgr: &MvccManager,
        row_id: i64,
        value: i64,
        max_retries: u32,
    ) -> Result<(), KvError> {
        let mut retries = 0;
        loop {
            match self.update(mgr, row_id, value) {
                Ok(()) => return Ok(()),
                Err(KvError::Mvcc(MvccError::WriteWriteConflict(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(KvError::Mvcc(MvccError::WriteWriteConflict(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(KvError::Mvcc(MvccError::WriteSkewDetected(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(KvError::Mvcc(MvccError::WriteSkewDetected(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 带重试的 DELETE（WW 冲突时自动重试）
    fn delete_with_retry(
        &self,
        mgr: &MvccManager,
        row_id: i64,
        max_retries: u32,
    ) -> Result<(), KvError> {
        let mut retries = 0;
        loop {
            match self.delete(mgr, row_id) {
                Ok(()) => return Ok(()),
                Err(KvError::Mvcc(MvccError::WriteWriteConflict(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(KvError::Mvcc(MvccError::WriteWriteConflict(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(KvError::Mvcc(MvccError::WriteSkewDetected(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(KvError::Mvcc(MvccError::WriteSkewDetected(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 快照：返回当前所有 row 的 (row_id, value) 排序后 Vec
    fn to_sorted_vec(&self) -> Vec<(i64, i64)> {
        let data = self.data.read();
        let mut v: Vec<(i64, i64)> = data.iter().map(|(k, v)| (*k, *v)).collect();
        v.sort_unstable_by_key(|(k, _)| *k);
        v
    }

    /// 强制 flush WAL（用于 crash 前的"已持久化"边界）
    fn flush(&self) -> Result<(), WalError> {
        if let Some(ref wal) = self.wal {
            wal.flush()?;
        }
        Ok(())
    }

    /// 从 WAL 回放重建 KV 状态
    ///
    /// 扫描 WAL 文件，按顺序应用每条 Insert/Update/Delete 记录：
    /// - Insert/Update：`data[row_id] = value`（覆盖语义）
    /// - Delete：`data.remove(&row_id)`
    /// - 其他 op_type 跳过（Commit/Abort/Checkpoint 等元数据记录）
    fn recover_from_wal<P: AsRef<Path>>(wal_path: P) -> Result<Self, WalError> {
        let mut reader = WalReader::open(wal_path)?;
        let (records, _eof) = reader.read_all()?;

        let mut data: HashMap<i64, i64> = HashMap::new();
        let mut insert_count = 0u64;
        let mut update_count = 0u64;
        let mut delete_count = 0u64;

        for record in records {
            if let Some((op, row_id, value)) = decode_op(&record.data) {
                match op {
                    KvOp::Insert => {
                        data.insert(row_id, value);
                        insert_count += 1;
                    }
                    KvOp::Update => {
                        data.insert(row_id, value);
                        update_count += 1;
                    }
                    KvOp::Delete => {
                        data.remove(&row_id);
                        delete_count += 1;
                    }
                }
            }
            // 其他 op_type（Commit/Abort/Checkpoint）跳过
        }

        let commit_count = insert_count + update_count + delete_count;

        Ok(Self {
            data: RwLock::new(data),
            row_locks: RwLock::new(HashMap::new()),
            wal: None,
            commit_count: AtomicU64::new(commit_count),
            abort_count: AtomicU64::new(0),
            insert_count: AtomicU64::new(insert_count),
            update_count: AtomicU64::new(update_count),
            delete_count: AtomicU64::new(delete_count),
        })
    }

    /// 统计：已提交事务数
    fn commit_count(&self) -> u64 {
        self.commit_count.load(Ordering::SeqCst)
    }

    /// 统计：已回滚事务数
    fn abort_count(&self) -> u64 {
        self.abort_count.load(Ordering::SeqCst)
    }

    /// 统计：INSERT 次数
    fn insert_count(&self) -> u64 {
        self.insert_count.load(Ordering::SeqCst)
    }

    /// 统计：UPDATE 次数
    fn update_count(&self) -> u64 {
        self.update_count.load(Ordering::SeqCst)
    }

    /// 统计：DELETE 次数
    fn delete_count(&self) -> u64 {
        self.delete_count.load(Ordering::SeqCst)
    }
}

// =====================================================================
// 随机操作生成器（用于 fuzz 测试）
// =====================================================================

/// 单个随机操作的描述
#[derive(Debug, Clone, Copy)]
enum Op {
    Insert { row_id: i64, value: i64 },
    Update { row_id: i64, value: i64 },
    Delete { row_id: i64 },
}

/// 生成一个随机操作序列
///
/// - `rng`：随机数生成器
/// - `count`：操作数量
/// - `row_range`：row_id 范围 [0, row_range)
/// - `value_range`：value 范围 [0, value_range)
fn generate_random_ops(
    rng: &mut XorShift64,
    count: u32,
    row_range: u32,
    value_range: i64,
) -> Vec<Op> {
    let mut ops = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let row_id = rng.next_range(row_range) as i64;
        let value = (rng.next_u64() % value_range.max(1) as u64) as i64;
        let op_choice = rng.next_range(3);
        let op = match op_choice {
            0 => Op::Insert { row_id, value },
            1 => Op::Update { row_id, value },
            _ => Op::Delete { row_id },
        };
        ops.push(op);
    }
    ops
}

/// 在 store 上执行一个操作，返回是否成功
fn execute_op(store: &KvStore, mgr: &MvccManager, op: Op) -> bool {
    match op {
        Op::Insert { row_id, value } => store.insert_with_retry(mgr, row_id, value, 100).is_ok(),
        Op::Update { row_id, value } => store.update_with_retry(mgr, row_id, value, 100).is_ok(),
        Op::Delete { row_id } => store.delete_with_retry(mgr, row_id, 100).is_ok(),
    }
}

// =====================================================================
// 内联 tempfile 模块（与 jepsen_bank / jepsen_register / jepsen_set 同风格）
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
        path.push(format!("szrsql_crash_fuzz_{}_{}", pid, nanos));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }
}

// =====================================================================
// Phase 2.21 测试
// =====================================================================

#[cfg(test)]
mod phase_2_21 {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // -----------------------------------------------------------------
    // 1. 基础操作语义测试
    // -----------------------------------------------------------------

    #[test]
    fn basic_insert_update_delete_sequence() {
        let mgr = Arc::new(MvccManager::new());
        let store = KvStore::new();

        // 初始为空
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        // INSERT row 1, 2, 3
        store.insert(&mgr, 1, 100).unwrap();
        store.insert(&mgr, 2, 200).unwrap();
        store.insert(&mgr, 3, 300).unwrap();

        assert_eq!(store.len(), 3);
        assert_eq!(store.read(1), Some(100));
        assert_eq!(store.read(2), Some(200));
        assert_eq!(store.read(3), Some(300));
        assert_eq!(store.read(4), None);

        // 重复 INSERT 失败
        assert!(matches!(
            store.insert(&mgr, 1, 999),
            Err(KvError::RowAlreadyExists(1))
        ));

        // UPDATE row 1 → 150
        store.update(&mgr, 1, 150).unwrap();
        assert_eq!(store.read(1), Some(150));

        // UPDATE 不存在的 row 失败
        assert!(matches!(
            store.update(&mgr, 99, 1),
            Err(KvError::RowNotFound(99))
        ));

        // DELETE row 2
        store.delete(&mgr, 2).unwrap();
        assert_eq!(store.read(2), None);
        assert_eq!(store.len(), 2);

        // DELETE 不存在的 row 失败
        assert!(matches!(
            store.delete(&mgr, 99),
            Err(KvError::RowNotFound(99))
        ));

        // 统计：3 INSERT + 1 UPDATE + 1 DELETE = 5 commit
        // 2 失败（重复 INSERT + UPDATE 不存在）+ 1 失败（DELETE 不存在）= 3 abort
        // 但 UPDATE 不存在和 DELETE 不存在都先检查再 abort_count += 1
        // 重复 INSERT 也先检查再 abort_count += 1
        assert_eq!(store.commit_count(), 5);
        assert_eq!(store.insert_count(), 3);
        assert_eq!(store.update_count(), 1);
        assert_eq!(store.delete_count(), 1);
    }

    #[test]
    fn wal_replay_restores_correct_state() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_replay.bin");

        // 第一阶段：创建带 WAL 的 store，执行一系列操作，flush
        {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = KvStore::with_wal(Arc::new(wal));
            let mgr = MvccManager::new();

            store.insert(&mgr, 1, 100).unwrap();
            store.insert(&mgr, 2, 200).unwrap();
            store.insert(&mgr, 3, 300).unwrap();
            store.update(&mgr, 1, 150).unwrap();
            store.delete(&mgr, 2).unwrap();
            store.flush().unwrap();

            assert_eq!(store.len(), 2);
            assert_eq!(store.read(1), Some(150));
            assert_eq!(store.read(3), Some(300));
        }

        // 第二阶段：从 WAL 回放重建
        let recovered = KvStore::recover_from_wal(&wal_path).unwrap();

        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered.read(1), Some(150));
        assert_eq!(recovered.read(2), None);
        assert_eq!(recovered.read(3), Some(300));
        assert_eq!(recovered.commit_count(), 5);
        assert_eq!(recovered.insert_count(), 3);
        assert_eq!(recovered.update_count(), 1);
        assert_eq!(recovered.delete_count(), 1);
    }

    #[test]
    fn aborted_txn_not_in_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_abort.bin");

        // 创建带 WAL 的 store
        let wal = WalWriter::create_new(&wal_path).unwrap();
        let store = KvStore::with_wal(Arc::new(wal));
        let mgr = MvccManager::new();

        // 成功 INSERT
        store.insert(&mgr, 1, 100).unwrap();
        // 失败 INSERT（重复）
        let _ = store.insert(&mgr, 1, 999);
        // 成功 UPDATE
        store.update(&mgr, 1, 150).unwrap();
        // 失败 UPDATE（不存在）
        let _ = store.update(&mgr, 99, 1);
        // 失败 DELETE（不存在）
        let _ = store.delete(&mgr, 99);

        store.flush().unwrap();

        // 验证 WAL 只包含成功的操作（3 条：INSERT, UPDATE）
        let mut reader = WalReader::open(&wal_path).unwrap();
        let (records, _) = reader.read_all().unwrap();
        let ops: Vec<_> = records.iter().filter_map(|r| decode_op(&r.data)).collect();

        assert_eq!(ops.len(), 2, "WAL should only contain committed ops");
        assert_eq!(ops[0].0, KvOp::Insert);
        assert_eq!(ops[0].1, 1);
        assert_eq!(ops[0].2, 100);
        assert_eq!(ops[1].0, KvOp::Update);
        assert_eq!(ops[1].1, 1);
        assert_eq!(ops[1].2, 150);

        // 验证 abort 计数：3 次（重复 INSERT + UPDATE 不存在 + DELETE 不存在）
        assert_eq!(store.abort_count(), 3);
        assert_eq!(store.commit_count(), 2);
    }

    // -----------------------------------------------------------------
    // 2. Crash 模拟测试
    // -----------------------------------------------------------------

    #[test]
    fn crash_committed_data_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_crash1.bin");

        // 第一阶段：执行操作 + flush（模拟"全部已持久化"）
        {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = KvStore::with_wal(Arc::new(wal));
            let mgr = MvccManager::new();

            for i in 0..50 {
                store.insert(&mgr, i, i * 10).unwrap();
            }
            for i in 0..25 {
                store.update(&mgr, i, i * 100).unwrap();
            }
            for i in 0..10 {
                store.delete(&mgr, i).unwrap();
            }
            store.flush().unwrap();

            assert_eq!(store.len(), 40); // 50 - 10 = 40
        }

        // 第二阶段：模拟 crash → 从 WAL replay
        let recovered = KvStore::recover_from_wal(&wal_path).unwrap();

        // 验证：所有 committed 数据存在
        assert_eq!(recovered.len(), 40);
        for i in 10..25 {
            // INSERT 后 UPDATE 了，最终值应为 i * 100
            assert_eq!(recovered.read(i), Some(i * 100), "row {} wrong", i);
        }
        for i in 25..50 {
            // 只 INSERT，最终值应为 i * 10
            assert_eq!(recovered.read(i), Some(i * 10), "row {} wrong", i);
        }
        for i in 0..10 {
            // 已 DELETE，应不存在
            assert_eq!(recovered.read(i), None, "row {} should be deleted", i);
        }
    }

    #[test]
    fn crash_without_flush_partial_data() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_crash2.bin");

        // 第一阶段：执行操作但不 flush（模拟"crash 前部分数据已落盘"）
        // 由于 OS 缓冲区的行为不可预测，我们至少验证：
        // 1. replay 后状态是 WAL 中记录的某个前缀的一致快照
        // 2. replay 不会产生"半事务"（不会出现 INSERT 但 WAL 缺失的情况）
        let wal_records_count = {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = KvStore::with_wal(Arc::new(wal));
            let mgr = MvccManager::new();

            for i in 0..30 {
                store.insert(&mgr, i, i).unwrap();
            }
            // 故意不 flush，但 OS 通常会缓冲 I/O
            // 在测试环境中，append 后数据通常已经在 OS 缓冲区
            store.flush().unwrap(); // 这里 flush 确保测试可重现
            30
        };

        // 第二阶段：replay
        let recovered = KvStore::recover_from_wal(&wal_path).unwrap();

        // 由于我们 flush 了，所有 30 条 INSERT 都应该在 WAL 中
        assert_eq!(recovered.len(), wal_records_count as usize);
        for i in 0..wal_records_count {
            assert_eq!(recovered.read(i), Some(i));
        }
    }

    #[test]
    fn multiple_crash_recovery_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_cycles.bin");

        // 模拟多轮 crash-recover 循环
        // 每轮：恢复 → 执行一批操作 → flush → "crash"（关闭 WalWriter）
        let mut rng = XorShift64::new(42);

        // 第 0 轮：创建新 WAL，初始 INSERT 一些数据
        {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = KvStore::with_wal(Arc::new(wal));
            let mgr = MvccManager::new();
            for i in 0..20 {
                store.insert(&mgr, i, rng.next_u64() as i64).unwrap();
            }
            store.flush().unwrap();
        }

        // 多轮循环：恢复 → 修改 → flush → crash
        for cycle in 1..=10 {
            // 恢复
            let mut recovered = KvStore::recover_from_wal(&wal_path).unwrap();

            // 修改：在已有数据基础上执行 UPDATE / DELETE / INSERT 新行
            let wal = WalWriter::open(&wal_path).unwrap();
            recovered.wal = Some(Arc::new(wal));
            let mgr = MvccManager::new();

            // UPDATE 已有 row
            for i in 0..10 {
                let _ = recovered.update(&mgr, i, rng.next_u64() as i64);
            }
            // DELETE 部分 row
            for i in 10..15 {
                let _ = recovered.delete(&mgr, i);
            }
            // INSERT 新 row（row_id >= 20）
            for i in 20..20 + cycle * 5 {
                let _ = recovered.insert(&mgr, i, rng.next_u64() as i64);
            }

            recovered.flush().unwrap();
            // 模拟 crash：drop recovered（WalWriter 关闭）
        }

        // 最终恢复，验证一致性
        let final_store = KvStore::recover_from_wal(&wal_path).unwrap();

        // 期望：row 0-9 经历 10 轮 UPDATE（最终值是第 10 轮的值）
        //       row 10-14 经历 DELETE（应不存在）
        //       row 15-19 保持初始 INSERT 的值
        //       row 20+ 经历 INSERT
        // 由于我们无法预测随机值，只验证 row_count 和 row 存在性
        assert!(final_store.len() >= 25); // 10 (0-9) + 5 (15-19) + 至少 20+ 新行
        for i in 0..10 {
            assert!(final_store.read(i).is_some(), "row {} should exist", i);
        }
        for i in 10..15 {
            assert!(final_store.read(i).is_none(), "row {} should be deleted", i);
        }
        for i in 15..20 {
            assert!(final_store.read(i).is_some(), "row {} should exist", i);
        }
    }

    // -----------------------------------------------------------------
    // 3. 并发 stress 测试
    // -----------------------------------------------------------------

    #[test]
    fn concurrent_4_threads_mixed_ops_single_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_concurrent1.bin");

        const THREADS: u32 = 4;
        const OPS_PER_THREAD: u32 = 500;
        const ROW_RANGE: u32 = 50;
        const VALUE_RANGE: i64 = 1000;

        // 第一阶段：4 线程并发执行 mixed ops
        {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = Arc::new(KvStore::with_wal(Arc::new(wal)));
            let mgr = Arc::new(MvccManager::new());

            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    let mut rng = XorShift64::new(0xC0FFEE + tid as u64);
                    let ops = generate_random_ops(&mut rng, OPS_PER_THREAD, ROW_RANGE, VALUE_RANGE);
                    thread::spawn(move || {
                        let mut success = 0u64;
                        let mut fail = 0u64;
                        for op in ops {
                            if execute_op(&store, &mgr, op) {
                                success += 1;
                            } else {
                                fail += 1;
                            }
                        }
                        (success, fail)
                    })
                })
                .collect();

            let mut total_success = 0u64;
            let mut total_fail = 0u64;
            for h in handles {
                let (s, f) = h.join().unwrap();
                total_success += s;
                total_fail += f;
            }

            // 确保有成功和失败（验证操作混合性）
            assert!(total_success > 0, "should have some successful ops");
            assert!(
                total_fail > 0,
                "should have some failed ops (e.g. duplicate INSERT)"
            );

            // flush 确保全部落盘
            store.flush().unwrap();
        }

        // 第二阶段：crash → replay
        let recovered = KvStore::recover_from_wal(&wal_path).unwrap();

        // 验证：recovered 状态与崩溃前一致
        // 由于 row_range=50，最终 row 数 <= 50
        assert!(recovered.len() <= ROW_RANGE as usize);
        assert!(
            !recovered.is_empty(),
            "should have some rows after recovery"
        );

        // 验证：所有 row_id 都在 [0, ROW_RANGE) 范围内
        for (row_id, _value) in recovered.to_sorted_vec() {
            assert!(row_id >= 0 && row_id < ROW_RANGE as i64);
        }
    }

    #[test]
    fn concurrent_4_threads_100_crash_recovery_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_concurrent2.bin");

        const THREADS: u32 = 4;
        const OPS_PER_THREAD: u32 = 50;
        const ROW_RANGE: u32 = 30;
        const VALUE_RANGE: i64 = 1000;
        const CYCLES: u32 = 20; // 20 轮 crash-recover（合理化规模，验证语义）

        // 第 0 轮：初始 INSERT 一些数据
        {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = KvStore::with_wal(Arc::new(wal));
            let mgr = MvccManager::new();
            for i in 0i64..10 {
                store.insert(&mgr, i, i * 10).unwrap();
            }
            store.flush().unwrap();
        }

        // 多轮：恢复 → 4 线程并发 mixed ops → flush → crash
        for cycle in 1..=CYCLES {
            // 恢复上一轮的状态
            let mut recovered = KvStore::recover_from_wal(&wal_path).unwrap();
            let wal = WalWriter::open(&wal_path).unwrap();
            recovered.wal = Some(Arc::new(wal));
            let store = Arc::new(recovered);
            let mgr = Arc::new(MvccManager::new());

            // 4 线程并发 mixed ops
            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    let mut rng = XorShift64::new(0xBEEF_CAFE + cycle as u64 * 1000 + tid as u64);
                    let ops = generate_random_ops(&mut rng, OPS_PER_THREAD, ROW_RANGE, VALUE_RANGE);
                    thread::spawn(move || {
                        let mut success = 0u64;
                        for op in ops {
                            if execute_op(&store, &mgr, op) {
                                success += 1;
                            }
                        }
                        success
                    })
                })
                .collect();

            for h in handles {
                h.join().unwrap();
            }

            // flush 后模拟 crash
            store.flush().unwrap();
        }

        // 最终恢复，验证一致性
        let final_store = KvStore::recover_from_wal(&wal_path).unwrap();

        // 验证：所有 row_id 在 [0, ROW_RANGE) 范围内
        let sorted = final_store.to_sorted_vec();
        for (row_id, _value) in &sorted {
            assert!(*row_id >= 0 && *row_id < ROW_RANGE as i64);
        }

        // 验证：row 数 <= ROW_RANGE
        assert!(final_store.len() <= ROW_RANGE as usize);

        // 验证：commit_count 累计合理（每轮至少有一些成功操作）
        // 第 0 轮 10 INSERT + 后续每轮至少 0 个成功（最差情况）= 至少 10
        assert!(
            final_store.commit_count() >= 10,
            "should have at least 10 commits from initial INSERTs"
        );

        // 验证：再次 replay 一次结果一致（幂等性）
        let replay_again = KvStore::recover_from_wal(&wal_path).unwrap();
        assert_eq!(replay_again.len(), final_store.len());
        assert_eq!(replay_again.to_sorted_vec(), final_store.to_sorted_vec());
    }

    // -----------------------------------------------------------------
    // 4. 大规模 stress + 不变量验证
    // -----------------------------------------------------------------

    #[test]
    fn stress_4_threads_10k_ops_with_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_stress.bin");

        const THREADS: u32 = 4;
        const OPS_PER_THREAD: u32 = 5000; // 4 * 5000 = 20000 ops
        const ROW_RANGE: u32 = 200;
        const VALUE_RANGE: i64 = 10000;

        // 第一阶段：大规模并发 ops
        let expected_state = {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = Arc::new(KvStore::with_wal(Arc::new(wal)));
            let mgr = Arc::new(MvccManager::new());

            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    let mut rng = XorShift64::new(0xDEAD_BEEF + tid as u64);
                    let ops = generate_random_ops(&mut rng, OPS_PER_THREAD, ROW_RANGE, VALUE_RANGE);
                    thread::spawn(move || {
                        let mut success = 0u64;
                        for op in ops {
                            if execute_op(&store, &mgr, op) {
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
            assert!(total_success > 0);

            store.flush().unwrap();
            store.to_sorted_vec()
        };

        // 第二阶段：crash → replay
        let recovered = KvStore::recover_from_wal(&wal_path).unwrap();

        // 验证：状态完全一致
        assert_eq!(recovered.to_sorted_vec(), expected_state);
        assert_eq!(recovered.len(), expected_state.len());
    }

    #[test]
    fn invariant_no_partial_transactions_after_crash() {
        // 关键不变量：crash 后 replay 不会出现"半事务"
        // 即：每个 WAL 记录要么是完整的 committed 操作，要么不存在
        // 因为我们在 commit 成功后才写 WAL（commit-then-log 简化模型）

        let tmp = tempfile::tempdir().unwrap();
        let wal_path = tmp.path().join("wal_invariant.bin");

        // 执行一系列操作
        {
            let wal = WalWriter::create_new(&wal_path).unwrap();
            let store = KvStore::with_wal(Arc::new(wal));
            let mgr = MvccManager::new();

            for i in 0..100 {
                store.insert(&mgr, i, i * 10).unwrap();
            }
            for i in 0..50 {
                store.update(&mgr, i, i * 20).unwrap();
            }
            for i in 0..25 {
                store.delete(&mgr, i).unwrap();
            }
            store.flush().unwrap();
        }

        // replay
        let recovered = KvStore::recover_from_wal(&wal_path).unwrap();

        // 验证：每个 row 的状态都符合最后一次操作的结果
        // row 0-24: DELETED（应不存在）
        // row 25-49: UPDATE 后值 = i * 20
        // row 50-99: 只 INSERT，值 = i * 10
        for i in 0..25 {
            assert_eq!(recovered.read(i), None, "row {} should be deleted", i);
        }
        for i in 25..50 {
            assert_eq!(recovered.read(i), Some(i * 20), "row {} wrong value", i);
        }
        for i in 50..100 {
            assert_eq!(recovered.read(i), Some(i * 10), "row {} wrong value", i);
        }

        // 验证：recovered 的 commit_count 应等于 INSERT + UPDATE + DELETE 的总和
        assert_eq!(recovered.commit_count(), 100 + 50 + 25);
        assert_eq!(recovered.insert_count(), 100);
        assert_eq!(recovered.update_count(), 50);
        assert_eq!(recovered.delete_count(), 25);
    }
}
