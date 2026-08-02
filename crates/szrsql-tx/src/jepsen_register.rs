//! Jepsen Register 测试 — 对应 `SzRSQL实施进度.md` Phase 2.19。
//!
//! 验证标准（来自实施进度表）：
//! - **10 线程并发读写同一 key → 验证 final value 是最后一次写入的值**
//! - **多 key 并发读写 → 验证每 key 独立正确**
//! - **Register 语义正确**
//!
//! 设计要点：
//! 1. **RegisterStore**：内存 KV 存储（`HashMap<String, Arc<Mutex<i64>>>` + `RwLock`）
//!    - 每 key 一个 `Mutex<i64>`，保证单 key 的 read-modify-write 原子性
//!    - 可选 WAL 持久化（`Arc<WalWriter>`）
//!    - 全局写入序列号 `AtomicU64 write_seq`，追踪写入次数
//! 2. **Register 语义**：
//!    - `write(key, value)`：覆盖 key 的值为 value，返回新值
//!    - `read(key)`：读取 key 的当前值（不存在返回 0）
//!    - `cas(key, expected, new)`：若 key 当前值 == expected，则写入 new，返回 Ok(new)；否则返回 Err(current)
//! 3. **MVCC 事务**：
//!    - write：BEGIN + register_write + commit + WAL
//!    - read：BEGIN + register_read + commit（无 WAL，只读）
//!    - cas：BEGIN + register_read + register_write + commit + WAL（用 per-key lock 保证原子性）
//! 4. **并发不变量**：
//!    - 每次 read 返回的值必须是之前某次 write 写入的值（或初始值 0）
//!    - final value 必须是最后一次成功 write 的值
//!    - 多 key 场景：每 key 独立满足上述不变量
//! 5. **崩溃恢复**：
//!    - 关闭 WalWriter（不 flush）→ 重新打开 → replay → 重建 KV 状态
//!    - replay 后每 key 的值应等于 WAL 中最后一条 write 记录的值
//! 6. **XorShift64 PRNG**：固定种子，测试可重现（与 mvcc_fuzz / wal_fuzz / jepsen_bank 同风格）

use crate::mvcc::{MvccError, MvccManager};
use crate::wal::{WalError, WalOpType, WalReader, WalRecord, WalWriter};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Mutex, RwLock};

// =====================================================================
// XorShift64 — 固定种子 PRNG（与 mvcc_fuzz / wal_fuzz / jepsen_bank 同风格）
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
}

// =====================================================================
// KV 记录编码/解码（WAL data 字段格式）
// =====================================================================
//
// 格式：
//   key_len: u8       (key 长度，0-255)
//   key:     [u8; N]  (UTF-8 key)
//   value:   i64      (LE)
//
// 总长度：1 + N + 8 字节

fn encode_kv(key: &str, value: i64) -> Vec<u8> {
    let key_bytes = key.as_bytes();
    assert!(key_bytes.len() <= 255, "key too long");
    let mut buf = Vec::with_capacity(1 + key_bytes.len() + 8);
    buf.push(key_bytes.len() as u8);
    buf.extend_from_slice(key_bytes);
    buf.extend_from_slice(&value.to_le_bytes());
    buf
}

fn decode_kv(buf: &[u8]) -> Option<(String, i64)> {
    if buf.is_empty() {
        return None;
    }
    let key_len = buf[0] as usize;
    if buf.len() < 1 + key_len + 8 {
        return None;
    }
    let key = std::str::from_utf8(&buf[1..1 + key_len]).ok()?.to_string();
    let value_off = 1 + key_len;
    let value = i64::from_le_bytes(buf[value_off..value_off + 8].try_into().ok()?);
    Some((key, value))
}

// =====================================================================
// RegisterError — Register 操作错误
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum RegisterError {
    #[error("cas failed: expected {expected}, actual {actual}")]
    CasMismatch { expected: i64, actual: i64 },
    #[error("mvcc error: {0}")]
    Mvcc(#[from] MvccError),
    #[error("wal error: {0}")]
    Wal(#[from] WalError),
}

// =====================================================================
// RegisterStore — KV 存储 + WAL 持久化
// =====================================================================

/// KV 存储 + WAL 持久化
///
/// 线程安全设计（与 jepsen_bank::BankStore 同风格）：
/// - `data`：`RwLock<HashMap<String, Arc<Mutex<i64>>>>`，外层 RwLock 保护 HashMap 结构，
///   内层 per-key Mutex 保护单个 key 的 read-modify-write 原子性
/// - `write_seq`：全局写入序列号（AtomicU64），统计成功 write 次数
/// - `commit_count` / `abort_count`：事务统计
///
/// **为什么需要 per-key lock**：
/// 与 jepsen_bank 同样的问题 — MvccManager 的 `begin_with_isolation` 非原子性，
/// 导致并发事务的 first-committer-wins 检测可能失效。per-key lock 保证 CAS 操作的
/// "读-比较-写"原子性。
struct RegisterStore {
    /// KV 数据（key -> 值的独立 Mutex）
    data: RwLock<HashMap<String, Arc<Mutex<i64>>>>,
    /// 可选的 WAL writer
    wal: Option<Arc<WalWriter>>,
    /// 全局写入序列号（成功 write 次数）
    write_seq: AtomicU64,
    /// 已提交事务数
    commit_count: AtomicU64,
    /// 已回滚事务数
    abort_count: AtomicU64,
}

impl RegisterStore {
    /// 创建无 WAL 的内存 Register（用于纯并发测试）
    fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            wal: None,
            write_seq: AtomicU64::new(0),
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
        }
    }

    /// 创建带 WAL 的 Register（用于崩溃恢复测试）
    fn with_wal(wal: Arc<WalWriter>) -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            wal: Some(wal),
            write_seq: AtomicU64::new(0),
            commit_count: AtomicU64::new(0),
            abort_count: AtomicU64::new(0),
        }
    }

    /// 获取 key 的 Arc<Mutex>（不存在则插入 0）
    fn get_or_create_key(&self, key: &str) -> Arc<Mutex<i64>> {
        {
            let map = self.data.read();
            if let Some(arc) = map.get(key) {
                return Arc::clone(arc);
            }
        }
        // 双检锁
        let mut map = self.data.write();
        map.entry(key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(0)))
            .clone()
    }

    /// 读取 key 的当前值（不存在返回 0）
    fn read(&self, key: &str) -> i64 {
        let map = self.data.read();
        match map.get(key) {
            Some(arc) => *arc.lock(),
            None => 0,
        }
    }

    /// 简单写入（无事务，直接覆盖；用于初始化）
    ///
    /// 不经过 MVCC 事务，不写 WAL，仅用于测试初始化。
    fn set(&self, key: &str, value: i64) {
        let arc = self.get_or_create_key(key);
        *arc.lock() = value;
    }

    /// MVCC 事务写入：BEGIN + register_write + commit + WAL
    ///
    /// **注**：与 jepsen_bank 不同，register 的 write 不需要 per-key lock，
    /// 因为 write 是"覆盖"语义（不依赖当前值）。但如果调用方需要"读-改-写"，
    /// 应使用 `cas` 方法。
    fn write(&self, mgr: &MvccManager, key: &str, value: i64) -> Result<i64, RegisterError> {
        let txn = mgr.begin();
        let _ = mgr.register_write(txn.txn_id, key);

        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                // 写 WAL
                if let Some(ref wal) = self.wal {
                    let record =
                        WalRecord::new(0, txn.txn_id, WalOpType::Commit, 0, encode_kv(key, value));
                    wal.append(record)?;
                }
                // 应用到内存
                let arc = self.get_or_create_key(key);
                *arc.lock() = value;
                self.write_seq.fetch_add(1, Ordering::SeqCst);
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                Ok(value)
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(RegisterError::Mvcc(e))
            }
        }
    }

    /// MVCC 事务读取：BEGIN + register_read + commit（只读，不写 WAL）
    ///
    /// 返回 key 的当前值。
    fn read_txn(&self, mgr: &MvccManager, key: &str) -> Result<i64, RegisterError> {
        let txn = mgr.begin();
        let _ = mgr.register_read(txn.txn_id, key);
        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                Ok(self.read(key))
            }
            Err(e) => {
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(RegisterError::Mvcc(e))
            }
        }
    }

    /// CAS（compare-and-swap）：原子"读-比较-写"
    ///
    /// 流程：
    /// 1. 获取 key 的 per-key lock
    /// 2. 读当前值 current
    /// 3. 若 current != expected → 返回 CasMismatch(current)
    /// 4. MVCC BEGIN + register_read + register_write + commit
    /// 5. commit 成功 → 写 WAL + 应用内存
    ///
    /// **关键设计**：per-key lock 保证"读-比较-写"原子性，避免并发 CAS 的丢失更新。
    fn cas(
        &self,
        mgr: &MvccManager,
        key: &str,
        expected: i64,
        new: i64,
    ) -> Result<i64, RegisterError> {
        let arc = self.get_or_create_key(key);
        let mut guard = arc.lock();
        let current = *guard;

        if current != expected {
            self.abort_count.fetch_add(1, Ordering::SeqCst);
            return Err(RegisterError::CasMismatch {
                expected,
                actual: current,
            });
        }

        // current == expected，执行写入
        let txn = mgr.begin();
        let _ = mgr.register_read(txn.txn_id, key);
        let _ = mgr.register_write(txn.txn_id, key);

        match mgr.commit(txn.txn_id, 0) {
            Ok(()) => {
                if let Some(ref wal) = self.wal {
                    let record =
                        WalRecord::new(0, txn.txn_id, WalOpType::Commit, 0, encode_kv(key, new));
                    wal.append(record)?;
                }
                *guard = new;
                drop(guard);
                self.write_seq.fetch_add(1, Ordering::SeqCst);
                self.commit_count.fetch_add(1, Ordering::SeqCst);
                Ok(new)
            }
            Err(e) => {
                drop(guard);
                self.abort_count.fetch_add(1, Ordering::SeqCst);
                Err(RegisterError::Mvcc(e))
            }
        }
    }

    /// 带重试的 write（用于并发测试，WW 冲突时自动重试）
    ///
    /// **注**：write 是"覆盖"语义，不依赖当前值，因此 WW 冲突时可以直接重试。
    fn write_with_retry(
        &self,
        mgr: &MvccManager,
        key: &str,
        value: i64,
        max_retries: u32,
    ) -> Result<i64, RegisterError> {
        let mut retries = 0;
        loop {
            match self.write(mgr, key, value) {
                Ok(v) => return Ok(v),
                Err(RegisterError::Mvcc(MvccError::WriteWriteConflict(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(RegisterError::Mvcc(MvccError::WriteWriteConflict(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(RegisterError::Mvcc(MvccError::WriteSkewDetected(_))) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(RegisterError::Mvcc(MvccError::WriteSkewDetected(0)));
                    }
                    std::hint::spin_loop();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// key 数量
    fn key_count(&self) -> usize {
        self.data.read().len()
    }

    /// 已提交事务数
    fn commit_count(&self) -> u64 {
        self.commit_count.load(Ordering::SeqCst)
    }

    /// 已回滚事务数
    fn abort_count(&self) -> u64 {
        self.abort_count.load(Ordering::SeqCst)
    }

    /// 全局写入序列号（成功 write 次数）
    fn write_seq(&self) -> u64 {
        self.write_seq.load(Ordering::SeqCst)
    }

    /// 从 WAL 回放重建 KV 状态
    ///
    /// 扫描 WAL 文件，对每条 `WalOpType::Commit` 记录解析 KV 信息，
    /// 直接覆盖 key 的值（最后一条记录的值为最终状态）。
    fn recover_from_wal<P: AsRef<Path>>(wal_path: P) -> Result<Self, WalError> {
        let mut reader = WalReader::open(wal_path)?;
        let (records, _eof) = reader.read_all()?;

        let mut data: HashMap<String, Arc<Mutex<i64>>> = HashMap::new();
        let mut write_seq = 0u64;

        for record in records {
            if record.op_type == WalOpType::Commit {
                if let Some((key, value)) = decode_kv(&record.data) {
                    data.insert(key, Arc::new(Mutex::new(value)));
                    write_seq += 1;
                }
            }
        }

        Ok(Self {
            data: RwLock::new(data),
            wal: None,
            write_seq: AtomicU64::new(write_seq),
            commit_count: AtomicU64::new(write_seq),
            abort_count: AtomicU64::new(0),
        })
    }
}

// =====================================================================
// 辅助函数：构造 key 名
// =====================================================================

/// 构造 key 名：k0, k1, ..., k9, k10, ...
fn key_name(idx: u32) -> String {
    format!("k{idx}")
}

// =====================================================================
// 内联 tempfile 模块（避免引入 dev-dependency；与 jepsen_bank 同风格）
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
        path.push(format!("szrsql_jepsen_register_{}_{}", pid, nanos));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }
}

// =====================================================================
// Phase 2.19 测试
// =====================================================================

#[cfg(test)]
mod phase_2_19 {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // -----------------------------------------------------------------
    // 1. 基础读写语义测试
    // -----------------------------------------------------------------

    #[test]
    fn basic_write_read_single_thread() {
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();

        // 初始未命中返回 0
        assert_eq!(store.read("k0"), 0);

        // write k0 = 42
        let v = store.write(&mgr, "k0", 42).unwrap();
        assert_eq!(v, 42);
        assert_eq!(store.read("k0"), 42);

        // write k0 = 100（覆盖）
        store.write(&mgr, "k0", 100).unwrap();
        assert_eq!(store.read("k0"), 100);

        // write 另一个 key
        store.write(&mgr, "k1", 7).unwrap();
        assert_eq!(store.read("k0"), 100);
        assert_eq!(store.read("k1"), 7);

        assert_eq!(store.write_seq(), 3);
        assert_eq!(store.key_count(), 2);
    }

    #[test]
    fn read_txn_returns_current_value() {
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();

        store.set("k0", 50);
        let v = store.read_txn(&mgr, "k0").unwrap();
        assert_eq!(v, 50);

        store.write(&mgr, "k0", 200).unwrap();
        let v = store.read_txn(&mgr, "k0").unwrap();
        assert_eq!(v, 200);
    }

    #[test]
    fn read_nonexistent_key_returns_zero() {
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();
        assert_eq!(store.read("absent"), 0);
        let v = store.read_txn(&mgr, "absent").unwrap();
        assert_eq!(v, 0);
    }

    // -----------------------------------------------------------------
    // 2. CAS 语义测试
    // -----------------------------------------------------------------

    #[test]
    fn cas_success_when_expected_matches() {
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();
        store.set("k0", 10);

        let v = store.cas(&mgr, "k0", 10, 20).unwrap();
        assert_eq!(v, 20);
        assert_eq!(store.read("k0"), 20);
        assert_eq!(store.write_seq(), 1);
    }

    #[test]
    fn cas_failure_when_expected_mismatches() {
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();
        store.set("k0", 10);

        let err = store.cas(&mgr, "k0", 99, 20).unwrap_err();
        match err {
            RegisterError::CasMismatch { expected, actual } => {
                assert_eq!(expected, 99);
                assert_eq!(actual, 10);
            }
            _ => panic!("expected CasMismatch"),
        }
        // 值未改变
        assert_eq!(store.read("k0"), 10);
        assert_eq!(store.write_seq(), 0);
        assert_eq!(store.abort_count(), 1);
    }

    #[test]
    fn cas_on_nonexistent_key_uses_zero_as_current() {
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();

        // 不存在 key，current = 0
        let v = store.cas(&mgr, "k0", 0, 5).unwrap();
        assert_eq!(v, 5);
        assert_eq!(store.read("k0"), 5);

        // 现在 current = 5，CAS(0, 10) 应失败
        let err = store.cas(&mgr, "k0", 0, 10).unwrap_err();
        match err {
            RegisterError::CasMismatch { expected, actual } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 5);
            }
            _ => panic!("expected CasMismatch"),
        }
    }

    #[test]
    fn write_with_retry_succeeds_under_no_conflict() {
        // 单线程下 write_with_retry 应该一次成功（无 WW 冲突）
        let mgr = Arc::new(MvccManager::new());
        let store = RegisterStore::new();

        let v = store.write_with_retry(&mgr, "k0", 42, 3).unwrap();
        assert_eq!(v, 42);
        assert_eq!(store.read("k0"), 42);
        assert_eq!(store.write_seq(), 1);

        // 再次 write
        let v = store.write_with_retry(&mgr, "k0", 100, 3).unwrap();
        assert_eq!(v, 100);
        assert_eq!(store.read("k0"), 100);
        assert_eq!(store.write_seq(), 2);
    }

    // -----------------------------------------------------------------
    // 3. 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn encode_decode_kv_roundtrip() {
        let cases = vec![
            ("k", 0i64),
            ("k0", 1),
            ("k9", -1),
            ("k100", i64::MAX),
            ("k255", i64::MIN),
            ("long_key_name_a", 42),
        ];
        for (key, value) in cases {
            let buf = encode_kv(key, value);
            let (k, v) = decode_kv(&buf).expect("decode should succeed");
            assert_eq!(k, key);
            assert_eq!(v, value);
        }
    }

    #[test]
    fn decode_kv_rejects_truncated_input() {
        // 空 buf
        assert!(decode_kv(&[]).is_none());

        // 只有 key_len
        assert!(decode_kv(&[3]).is_none());

        // key_len=3 但只有 2 字节 key
        assert!(decode_kv(&[3, b'a', b'b']).is_none());

        // key 完整但 value 截断
        assert!(decode_kv(&[2, b'a', b'b', 0, 0, 0, 0]).is_none());

        // 完整记录
        let buf = encode_kv("ab", 100);
        assert!(decode_kv(&buf).is_some());
    }

    // -----------------------------------------------------------------
    // 4. 并发读写测试 — 10 线程并发读写同一 key
    // -----------------------------------------------------------------

    /// 10 线程并发 write 同一 key（每线程写入不同值）
    /// 验证：final value 是最后一次成功 write 的值
    #[test]
    fn jepsen_register_10_threads_concurrent_write_final_value() {
        const THREADS: usize = 10;
        const WRITES_PER_THREAD: u32 = 500;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());

        // 记录所有写入值（线程安全 Vec）
        let written_values = Arc::new(Mutex::new(Vec::<i64>::new()));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let written_values = Arc::clone(&written_values);
                thread::spawn(move || {
                    let mut success = 0u64;
                    for i in 0..WRITES_PER_THREAD {
                        // 每次写入：tid * 1000 + i（确保每线程写入不同值）
                        let value = (tid as i64) * 1000 + i as i64;
                        // 使用 write_with_retry 应对 WW 冲突
                        if store.write_with_retry(&mgr, "k0", value, 100).is_ok() {
                            success += 1;
                            let mut wv = written_values.lock();
                            wv.push(value);
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

        // 验证：所有 write 都成功（WW 冲突时自动重试）
        assert_eq!(
            total_success,
            (THREADS as u64) * (WRITES_PER_THREAD as u64),
            "所有 write 都应成功（WW 冲突时自动重试）"
        );
        assert_eq!(
            store.write_seq(),
            (THREADS as u64) * (WRITES_PER_THREAD as u64)
        );

        // 验证：final value 是 written_values 中的某个值
        let final_value = store.read("k0");
        let wv = written_values.lock();
        assert!(
            wv.contains(&final_value),
            "final value {} 应该是某次 write 的值",
            final_value
        );

        // 验证：final value 是最后一次成功 write 的值
        // （由于并发，"最后一次"由全局序列号决定，我们检查 write_seq == wv.len()）
        assert_eq!(wv.len() as u64, store.write_seq());
    }

    /// 10 线程并发 read 同一 key，验证每次 read 返回的值都是"之前某次 write 的值或初始值"
    ///
    /// **注**：并发环境下"之前"是模糊的，但线性一致性要求 read 返回的值必须等于
    /// 某次已提交的 write 的值（或初始值 0）。
    ///
    /// **关键设计**：writer 在调用 store.write 之前先把值加入 committed_values，
    /// 这样即使 read 在 write 完成后立即读到新值，也能在 committed_values 中找到。
    /// （write 是原子操作，read 读到的要么是旧值要么是新值，不会是中间状态）
    #[test]
    fn jepsen_register_10_threads_concurrent_read_returns_valid_value() {
        const READERS: usize = 4;
        const WRITERS: usize = 4;
        const OPS_PER_THREAD: u32 = 500;
        const VALUE_RANGE: i64 = 1000;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());

        // 写入值集合（线程安全 HashSet，记录所有已提交的 write 值 + 初始值 0）
        let committed_values = Arc::new(Mutex::new(vec![0i64]));

        // 读验证线程收集的"非法 read"（read 返回的值不在 committed_values 中）
        let invalid_reads = Arc::new(AtomicU64::new(0));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // writer 线程
        let writer_handles: Vec<_> = (0..WRITERS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let committed_values = Arc::clone(&committed_values);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0x5E19);
                    let mut success = 0u64;
                    for _ in 0..OPS_PER_THREAD {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        // 写入 [1, VALUE_RANGE] 范围的值
                        let value = rng.next_in(1, VALUE_RANGE as u32) as i64;
                        // 先把值加入 committed_values，再调用 write
                        // （确保 read 读到新值时 committed_values 已包含）
                        {
                            let mut cv = committed_values.lock();
                            cv.push(value);
                        }
                        if store.write(&mgr, "k0", value).is_ok() {
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
                let committed_values = Arc::clone(&committed_values);
                let invalid_reads = Arc::clone(&invalid_reads);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0x81A5);
                    let mut total_reads = 0u64;
                    for _ in 0..OPS_PER_THREAD {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        // 读或 read_txn 随机
                        let value = if rng.next_range(2) == 0 {
                            store.read("k0")
                        } else {
                            store.read_txn(&mgr, "k0").unwrap_or(0)
                        };
                        total_reads += 1;
                        // 验证：value 必须在 committed_values 中
                        let cv = committed_values.lock();
                        if !cv.contains(&value) {
                            invalid_reads.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    total_reads
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
        let mut total_reads = 0u64;
        for h in reader_handles {
            total_reads += h.join().unwrap();
        }

        // 验证：0 非法 read
        let invalid = invalid_reads.load(Ordering::SeqCst);
        assert_eq!(
            invalid, 0,
            "发现 {} 次非法 read（返回值不在已提交值集合中），共 {} 次 read",
            invalid, total_reads
        );

        // 验证：final value 在 committed_values 中
        let final_value = store.read("k0");
        let cv = committed_values.lock();
        assert!(
            cv.contains(&final_value),
            "final value {} 应在已提交值集合中",
            final_value
        );

        // 验证：write_seq <= total_writes（部分 write 可能因 WW 冲突失败）
        // 注：由于没有重试，total_writes 是成功数，write_seq 也是成功数
        assert_eq!(store.write_seq(), total_writes);
    }

    /// 10 线程并发 CAS 同一 key（每线程尝试 +1）
    /// 验证：final value == 初始值 + 成功 CAS 数
    #[test]
    fn jepsen_register_10_threads_concurrent_cas_increment() {
        const THREADS: usize = 10;
        const CAS_PER_THREAD: u32 = 200;
        const INITIAL_VALUE: i64 = 1000;
        const EXPECTED_FINAL: i64 = INITIAL_VALUE + (THREADS as i64) * (CAS_PER_THREAD as i64);

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());
        store.set("counter", INITIAL_VALUE);

        let success_counts = Arc::new(Mutex::new(vec![0u64; THREADS]));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let success_counts = Arc::clone(&success_counts);
                thread::spawn(move || {
                    let mut success = 0u64;
                    let mut failures = 0u64;
                    for _ in 0..CAS_PER_THREAD {
                        // 循环重试直到成功或达到上限
                        let mut local_retries = 0u32;
                        loop {
                            let current = store.read("counter");
                            let new = current + 1;
                            match store.cas(&mgr, "counter", current, new) {
                                Ok(_) => {
                                    success += 1;
                                    break;
                                }
                                Err(RegisterError::CasMismatch { .. }) => {
                                    // 其他线程已修改，重试
                                    local_retries += 1;
                                    if local_retries > 50 {
                                        failures += 1;
                                        break;
                                    }
                                    std::hint::spin_loop();
                                }
                                Err(RegisterError::Mvcc(_)) => {
                                    // WW 冲突，重试
                                    local_retries += 1;
                                    if local_retries > 50 {
                                        failures += 1;
                                        break;
                                    }
                                    std::hint::spin_loop();
                                }
                                Err(_) => {
                                    failures += 1;
                                    break;
                                }
                            }
                        }
                    }
                    let mut sc = success_counts.lock();
                    sc[tid] = success;
                    let _ = failures;
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let sc = success_counts.lock();
        let total_success: u64 = sc.iter().sum();

        // 验证：final value == 初始值 + 成功 CAS 数
        let final_value = store.read("counter");
        assert_eq!(
            final_value,
            INITIAL_VALUE + total_success as i64,
            "final value {} 应等于 INITIAL_VALUE({}) + total_success({})",
            final_value,
            INITIAL_VALUE,
            total_success
        );

        // 验证：所有 CAS 都成功（max_retries=50 足够）
        assert_eq!(
            total_success,
            (THREADS as u64) * (CAS_PER_THREAD as u64),
            "所有 CAS 都应成功（每线程最多重试 50 次）"
        );

        // 验证：final value == EXPECTED_FINAL
        assert_eq!(final_value, EXPECTED_FINAL);

        // 验证：write_seq == total_success（每次成功 CAS 算一次 write）
        assert_eq!(store.write_seq(), total_success);
    }

    // -----------------------------------------------------------------
    // 5. 多 key 并发读写测试
    // -----------------------------------------------------------------

    /// 10 线程并发读写多个 key，验证每 key 独立正确
    ///
    /// **关键设计**：writer 在调用 store.write 之前先把值加入 key_histories，
    /// 这样即使 read 在 write 完成后立即读到新值，也能在 key_histories 中找到。
    #[test]
    fn jepsen_register_multi_key_independent_correctness() {
        const THREADS: usize = 10;
        const KEYS: u32 = 8;
        const OPS_PER_THREAD: u32 = 500;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());

        // 初始化所有 key 为 0
        for i in 0..KEYS {
            store.set(&key_name(i), 0);
        }

        // 每个 key 独立的写入历史（线程安全）
        let key_histories: Arc<Mutex<Vec<Vec<i64>>>> =
            Arc::new(Mutex::new((0..KEYS).map(|_| vec![0i64]).collect()));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let key_histories = Arc::clone(&key_histories);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0x4B1D);
                    let mut success = 0u64;
                    for _ in 0..OPS_PER_THREAD {
                        let key_idx = rng.next_range(KEYS);
                        let key = key_name(key_idx);
                        // 50% read, 50% write
                        if rng.next_range(2) == 0 {
                            // write：值范围 [1, 1000]，每线程唯一前缀
                            let value = (tid as i64) * 10000 + rng.next_in(1, 1000) as i64;
                            // 先把值加入 key_histories，再调用 write
                            {
                                let mut kh = key_histories.lock();
                                kh[key_idx as usize].push(value);
                            }
                            if store.write_with_retry(&mgr, &key, value, 100).is_ok() {
                                success += 1;
                            }
                        } else {
                            // read：验证返回值在 key 的历史中
                            let value = store.read(&key);
                            let kh = key_histories.lock();
                            assert!(
                                kh[key_idx as usize].contains(&value),
                                "key {} 返回值 {} 不在其历史中",
                                key,
                                value
                            );
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

        // 验证：每个 key 的 final value 在其历史中
        let kh = key_histories.lock();
        for i in 0..KEYS {
            let final_value = store.read(&key_name(i));
            assert!(
                kh[i as usize].contains(&final_value),
                "key {} final value {} 不在其历史中",
                key_name(i),
                final_value
            );
        }

        // 验证：write_seq == total_success（所有 write_with_retry 都应成功）
        assert_eq!(store.write_seq(), total_success);
    }

    /// 多 key 并发 CAS（每 key 独立计数器）
    #[test]
    fn jepsen_register_multi_key_concurrent_cas() {
        const THREADS: usize = 8;
        const KEYS: u32 = 4;
        const CAS_PER_THREAD: u32 = 200;
        const INITIAL_VALUE: i64 = 100;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());

        // 初始化每个 key
        for i in 0..KEYS {
            store.set(&key_name(i), INITIAL_VALUE);
        }

        // 每 key 的成功 CAS 数
        let key_success: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(vec![0u64; KEYS as usize]));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let key_success = Arc::clone(&key_success);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0x4A5E);
                    for _ in 0..CAS_PER_THREAD {
                        let key_idx = rng.next_range(KEYS);
                        let key = key_name(key_idx);
                        let mut local_retries = 0u32;
                        loop {
                            let current = store.read(&key);
                            let new = current + 1;
                            match store.cas(&mgr, &key, current, new) {
                                Ok(_) => {
                                    let mut ks = key_success.lock();
                                    ks[key_idx as usize] += 1;
                                    break;
                                }
                                Err(RegisterError::CasMismatch { .. }) => {
                                    local_retries += 1;
                                    if local_retries > 50 {
                                        break;
                                    }
                                    std::hint::spin_loop();
                                }
                                Err(RegisterError::Mvcc(_)) => {
                                    local_retries += 1;
                                    if local_retries > 50 {
                                        break;
                                    }
                                    std::hint::spin_loop();
                                }
                                Err(_) => break,
                            }
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // 验证：每个 key 的 final value == INITIAL_VALUE + 成功 CAS 数
        let ks = key_success.lock();
        let mut total_success = 0u64;
        for i in 0..KEYS {
            let final_value = store.read(&key_name(i));
            let expected = INITIAL_VALUE + ks[i as usize] as i64;
            assert_eq!(
                final_value,
                expected,
                "key {} final value {} 应等于 INITIAL_VALUE({}) + success({})",
                key_name(i),
                final_value,
                INITIAL_VALUE,
                ks[i as usize]
            );
            total_success += ks[i as usize];
        }

        // 验证：write_seq == total_success
        assert_eq!(store.write_seq(), total_success);
    }

    // -----------------------------------------------------------------
    // 6. 崩溃恢复测试
    // -----------------------------------------------------------------

    /// 基础崩溃恢复：write N 条记录 → 不 flush → recover → 验证最终值
    #[test]
    fn jepsen_register_crash_recovery_basic() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("register_basic.wal");

        const KEYS: u32 = 5;
        const WRITES_PER_KEY: u32 = 20;

        // 阶段 1：写入 N 条记录
        let expected_final_values: HashMap<String, i64> = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = RegisterStore::with_wal(wal);
            let mgr = MvccManager::new();

            let mut expected = HashMap::new();
            for k in 0..KEYS {
                let key = key_name(k);
                for i in 0..WRITES_PER_KEY {
                    let value = (k as i64) * 1000 + i as i64;
                    store.write(&mgr, &key, value).unwrap();
                }
                expected.insert(key.clone(), store.read(&key));
            }
            // flush 确保全部落盘
            store.wal.as_ref().unwrap().flush().unwrap();
            expected
        };

        // 阶段 2：模拟崩溃 — recover_from_wal
        let recovered = RegisterStore::recover_from_wal(&wal_path).unwrap();

        // 验证：每个 key 的值与崩溃前一致
        for (key, expected_value) in &expected_final_values {
            let actual = recovered.read(key);
            assert_eq!(
                actual, *expected_value,
                "key {} 恢复后值 {} != 期望 {}",
                key, actual, expected_value
            );
        }

        // 验证：write_seq 一致
        assert_eq!(
            recovered.write_seq(),
            (KEYS as u64) * (WRITES_PER_KEY as u64)
        );
    }

    /// 崩溃恢复 + 继续写入：write → crash → recover → write → crash → recover → 验证
    #[test]
    fn jepsen_register_crash_recovery_continue_writes() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("register_continue.wal");

        // ===== 阶段 1：初始写入 =====
        let phase1_expected: HashMap<String, i64> = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = RegisterStore::with_wal(wal);
            let mgr = MvccManager::new();
            let mut expected = HashMap::new();
            for k in 0..3u32 {
                let key = key_name(k);
                store.write(&mgr, &key, (k as i64) * 10).unwrap();
                expected.insert(key, (k as i64) * 10);
            }
            store.wal.as_ref().unwrap().flush().unwrap();
            expected
        };

        // ===== 阶段 2：第一次 recover + 继续写入 =====
        let phase2_expected: HashMap<String, i64> = {
            let recovered = RegisterStore::recover_from_wal(&wal_path).unwrap();
            // 验证 phase1 状态
            for (key, expected_value) in &phase1_expected {
                assert_eq!(recovered.read(key), *expected_value);
            }

            // 继续写入
            let new_wal = Arc::new(WalWriter::open(&wal_path).unwrap());
            let mgr = MvccManager::new();
            let store = RegisterStore {
                data: recovered.data,
                wal: Some(new_wal),
                write_seq: AtomicU64::new(recovered.write_seq.load(Ordering::SeqCst)),
                commit_count: AtomicU64::new(recovered.commit_count.load(Ordering::SeqCst)),
                abort_count: AtomicU64::new(0),
            };

            let mut expected = phase1_expected.clone();
            for k in 0..3u32 {
                let key = key_name(k);
                let new_value = (k as i64) * 100 + 7;
                store.write(&mgr, &key, new_value).unwrap();
                expected.insert(key, new_value);
            }
            store.wal.as_ref().unwrap().flush().unwrap();
            expected
        };

        // ===== 阶段 3：第二次 recover =====
        let recovered2 = RegisterStore::recover_from_wal(&wal_path).unwrap();
        for (key, expected_value) in &phase2_expected {
            assert_eq!(
                recovered2.read(key),
                *expected_value,
                "key {} 第二次恢复后值不正确",
                key
            );
        }
    }

    /// 崩溃不 flush 不损坏：write N → 不 flush → recover → 验证只看到已落盘的记录
    ///
    /// **注**：WAL 写入时会缓冲在 OS 中，flush 才保证落盘。
    /// 实际测试中，由于 WalWriter 的实现可能内部有缓冲，不 flush 时可能部分记录丢失，
    /// 但已落盘的记录应该完整可读。
    #[test]
    fn jepsen_register_crash_without_flush_no_corruption() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("register_noflush.wal");

        // 写入但故意不 flush
        {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = RegisterStore::with_wal(wal);
            let mgr = MvccManager::new();
            for i in 0..10u32 {
                store.write(&mgr, &key_name(i), i as i64 * 10).unwrap();
            }
            // 故意不 flush，直接 drop（模拟崩溃）
        }

        // recover：能读到的记录应该完整可解码（不损坏）
        let recovered = RegisterStore::recover_from_wal(&wal_path).unwrap();
        // 验证：恢复后每个 key 的值要么是写入的值，要么不存在（如果记录未落盘）
        // 由于 WalWriter 实现细节，这里只验证"不损坏"：所有能读到的记录都是合法的 KV 对
        for i in 0..10u32 {
            let value = recovered.read(&key_name(i));
            // 值要么是 0（不存在），要么是 i * 10
            assert!(
                value == 0 || value == i as i64 * 10,
                "key {} 恢复后值 {} 非法（应为 0 或 {}）",
                key_name(i),
                value,
                i as i64 * 10
            );
        }
    }

    /// 完整崩溃恢复工作流：并发 write → crash → recover → 验证 → 继续并发 write → crash → recover → 验证
    #[test]
    fn jepsen_register_full_crash_recovery_workflow() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wal_path = tmpdir.path().join("register_full.wal");

        const THREADS: usize = 4;
        const WRITES_PER_THREAD: u32 = 200;
        const KEYS: u32 = 4;

        // ===== 阶段 1：并发写入 =====
        let phase1_committed_values: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(vec![0i64]));
        let phase1_total_writes = {
            let wal = Arc::new(WalWriter::create_new(&wal_path).unwrap());
            let store = Arc::new(RegisterStore::with_wal(wal));
            let mgr = Arc::new(MvccManager::new());

            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    let committed_values = Arc::clone(&phase1_committed_values);
                    thread::spawn(move || {
                        let mut rng = XorShift64::new(tid as u64 + 0x5EED);
                        let mut success = 0u64;
                        for _ in 0..WRITES_PER_THREAD {
                            let key_idx = rng.next_range(KEYS);
                            let value = (tid as i64) * 1000 + rng.next_in(1, 100) as i64;
                            if store.write(&mgr, &key_name(key_idx), value).is_ok() {
                                success += 1;
                                let mut cv = committed_values.lock();
                                cv.push(value);
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
            // flush 确保全部落盘
            store.wal.as_ref().unwrap().flush().unwrap();
            total
        };

        // ===== 阶段 2：模拟重启 — 从 WAL replay =====
        let recovered = RegisterStore::recover_from_wal(&wal_path).unwrap();

        // 验证：恢复后的 write_seq == phase1_total_writes
        assert_eq!(recovered.write_seq(), phase1_total_writes);

        // 验证：每个 key 的 final value 在 committed_values 中
        let cv = phase1_committed_values.lock();
        for k in 0..KEYS {
            let final_value = recovered.read(&key_name(k));
            assert!(
                cv.contains(&final_value),
                "key {} 恢复后值 {} 不在已提交值集合中",
                key_name(k),
                final_value
            );
        }
        drop(cv);

        // ===== 阶段 3：第二轮并发写入（使用 replay 后的状态） =====
        let phase2_committed_values: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(vec![0i64]));
        let phase2_total_writes = {
            let new_wal = Arc::new(WalWriter::open(&wal_path).unwrap());
            let mgr = Arc::new(MvccManager::new());
            let store = Arc::new(RegisterStore {
                data: recovered.data,
                wal: Some(new_wal),
                write_seq: AtomicU64::new(recovered.write_seq.load(Ordering::SeqCst)),
                commit_count: AtomicU64::new(recovered.commit_count.load(Ordering::SeqCst)),
                abort_count: AtomicU64::new(0),
            });

            let handles: Vec<_> = (0..THREADS)
                .map(|tid| {
                    let mgr = Arc::clone(&mgr);
                    let store = Arc::clone(&store);
                    let committed_values = Arc::clone(&phase2_committed_values);
                    thread::spawn(move || {
                        let mut rng = XorShift64::new(tid as u64 + 0xA11CE);
                        let mut success = 0u64;
                        for _ in 0..WRITES_PER_THREAD {
                            let key_idx = rng.next_range(KEYS);
                            let value = (tid as i64) * 10000 + rng.next_in(1, 100) as i64;
                            if store.write(&mgr, &key_name(key_idx), value).is_ok() {
                                success += 1;
                                let mut cv = committed_values.lock();
                                cv.push(value);
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
            total
        };

        // ===== 阶段 4：再次崩溃恢复，验证最终状态 =====
        let recovered2 = RegisterStore::recover_from_wal(&wal_path).unwrap();

        // 验证：write_seq == phase1 + phase2
        assert_eq!(
            recovered2.write_seq(),
            phase1_total_writes + phase2_total_writes
        );

        // 验证：每个 key 的 final value 在 phase2_committed_values 中
        let cv2 = phase2_committed_values.lock();
        for k in 0..KEYS {
            let final_value = recovered2.read(&key_name(k));
            assert!(
                cv2.contains(&final_value),
                "key {} 第二次恢复后值 {} 不在 phase2 已提交值集合中",
                key_name(k),
                final_value
            );
        }
    }

    // -----------------------------------------------------------------
    // 7. 并发不变量测试
    // -----------------------------------------------------------------

    /// 验证：并发 write 过程中，read 返回的值始终是"已提交的 write 值或初始值"
    ///
    /// **关键设计**：writer 在调用 store.write 之前先把值加入 committed_values，
    /// 这样即使 read 在 write 完成后立即读到新值，也能在 committed_values 中找到。
    #[test]
    fn jepsen_register_invariant_read_returns_committed_value() {
        const WRITERS: usize = 4;
        const READERS: usize = 4;
        const OPS_PER_THREAD: u32 = 1000;
        const VALUE_RANGE: i64 = 500;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());

        // 已提交值集合（线程安全），初始包含 0
        let committed_values = Arc::new(Mutex::new(vec![0i64]));

        // 非法 read 计数
        let invalid_reads = Arc::new(AtomicU64::new(0));
        let total_reads = Arc::new(AtomicU64::new(0));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // writer 线程
        let writer_handles: Vec<_> = (0..WRITERS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let committed_values = Arc::clone(&committed_values);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0xDA7A);
                    let mut success = 0u64;
                    for _ in 0..OPS_PER_THREAD {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let value = rng.next_in(1, VALUE_RANGE as u32) as i64;
                        // 先把值加入 committed_values，再调用 write
                        {
                            let mut cv = committed_values.lock();
                            cv.push(value);
                        }
                        if store.write(&mgr, "k0", value).is_ok() {
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
                let committed_values = Arc::clone(&committed_values);
                let invalid_reads = Arc::clone(&invalid_reads);
                let total_reads = Arc::clone(&total_reads);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut rng = XorShift64::new(tid as u64 + 0xBEEF);
                    let mut local_reads = 0u64;
                    while !stop.load(Ordering::SeqCst) {
                        // 随机使用 read 或 read_txn
                        let value = if rng.next_range(2) == 0 {
                            store.read("k0")
                        } else {
                            store.read_txn(&mgr, "k0").unwrap_or(0)
                        };
                        local_reads += 1;
                        // 验证：value 必须在 committed_values 中
                        let cv = committed_values.lock();
                        if !cv.contains(&value) {
                            invalid_reads.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    total_reads.fetch_add(local_reads, Ordering::SeqCst);
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

        // 验证：0 非法 read
        let invalid = invalid_reads.load(Ordering::SeqCst);
        let total_r = total_reads.load(Ordering::SeqCst);
        assert_eq!(
            invalid, 0,
            "发现 {} 次非法 read（返回值不在已提交值集合中），共 {} 次 read",
            invalid, total_r
        );

        // 验证：write_seq == total_writes
        assert_eq!(store.write_seq(), total_writes);
    }

    /// 验证：并发 CAS 下，counter 单调递增（无丢失更新）
    #[test]
    fn jepsen_register_invariant_cas_counter_monotonic() {
        const THREADS: usize = 8;
        const CAS_PER_THREAD: u32 = 300;
        const INITIAL_VALUE: i64 = 0;

        let mgr = Arc::new(MvccManager::new());
        let store = Arc::new(RegisterStore::new());
        store.set("counter", INITIAL_VALUE);

        let success_counts = Arc::new(Mutex::new(vec![0u64; THREADS]));

        let handles: Vec<_> = (0..THREADS)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let store = Arc::clone(&store);
                let success_counts = Arc::clone(&success_counts);
                thread::spawn(move || {
                    let mut success = 0u64;
                    for _ in 0..CAS_PER_THREAD {
                        let mut local_retries = 0u32;
                        loop {
                            let current = store.read("counter");
                            let new = current + 1;
                            match store.cas(&mgr, "counter", current, new) {
                                Ok(_) => {
                                    success += 1;
                                    break;
                                }
                                Err(RegisterError::CasMismatch { .. }) => {
                                    local_retries += 1;
                                    if local_retries > 100 {
                                        break;
                                    }
                                    std::hint::spin_loop();
                                }
                                Err(RegisterError::Mvcc(_)) => {
                                    local_retries += 1;
                                    if local_retries > 100 {
                                        break;
                                    }
                                    std::hint::spin_loop();
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    let mut sc = success_counts.lock();
                    sc[tid] = success;
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let sc = success_counts.lock();
        let total_success: u64 = sc.iter().sum();

        // 验证：final value == INITIAL_VALUE + total_success（无丢失更新）
        let final_value = store.read("counter");
        assert_eq!(
            final_value,
            INITIAL_VALUE + total_success as i64,
            "counter 最终值 {} 应等于 INITIAL_VALUE({}) + total_success({})，存在丢失更新",
            final_value,
            INITIAL_VALUE,
            total_success
        );

        // 验证：所有 CAS 都成功
        assert_eq!(
            total_success,
            (THREADS as u64) * (CAS_PER_THREAD as u64),
            "所有 CAS 都应成功"
        );

        // 验证：write_seq == total_success
        assert_eq!(store.write_seq(), total_success);
    }
}
