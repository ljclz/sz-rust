//! Phase 8.7：Percolator 分布式事务
//!
//! 基于 Google Percolator 论文（"Large-scale Incremental Processing Using
//! Distributed Transactions and Notifications", OSDI 2012）实现的分布式事务协议。
//!
//! # 核心思想
//!
//! - **时间戳服务（TSO）**：全局单调递增的时间戳，用于事务排序
//! - **两阶段提交（2PC）**：
//!   - Phase 1 (Prewrite)：对所有写入键加锁 + 写入数据版本
//!   - Phase 2 (Commit)：先提交主键，再提交次键，提交后释放锁
//! - **故障恢复**：客户端崩溃后，未完成事务的锁残留，其他事务遇到时
//!   通过检查主键状态决定回滚（ROLLBACK）或前推（COMMIT）
//!
//! # 数据布局
//!
//! 在底层 KV 存储中，每个原始键 K 关联三类记录：
//!
//! - **数据记录**：`DATA_PREFIX || K || start_ts` -> value
//! - **锁记录**：`LOCK_PREFIX || K` -> (primary_key, start_ts, kind, value)
//! - **写记录**：`WRITE_PREFIX || K || commit_ts` -> (start_ts, kind)
//!
//! 通过键前缀和范围扫描实现多版本和锁检查。

use crate::raft::RaftError;
use crate::shard::{KeyRange, ShardCluster, ShardId};
use std::fmt;

// =====================================================================
//  常量
// =====================================================================

/// 数据记录前缀
pub(crate) const DATA_PREFIX: u8 = 0x01;
/// 锁记录前缀
pub(crate) const LOCK_PREFIX: u8 = 0x02;
/// 写记录前缀
pub(crate) const WRITE_PREFIX: u8 = 0x03;

/// 锁类型：Put
pub(crate) const LOCK_KIND_PUT: u8 = 0x01;
/// 锁类型：Delete
pub(crate) const LOCK_KIND_DELETE: u8 = 0x02;

/// 写记录类型：Put
pub(crate) const WRITE_KIND_PUT: u8 = 0x01;
/// 写记录类型：Delete
pub(crate) const WRITE_KIND_DELETE: u8 = 0x02;
/// 写记录类型：Rollback
pub(crate) const WRITE_KIND_ROLLBACK: u8 = 0x03;

// =====================================================================
//  TimestampOracle — 全局时间戳服务
// =====================================================================

/// 全局单调递增时间戳服务（TSO）。
///
/// 模拟 Percolator 中的 Timestamp Oracle，所有事务通过它获取
/// start_ts（事务开始时）和 commit_ts（提交时）。
#[derive(Debug, Default)]
pub struct TimestampOracle {
    /// 当前已分配的最大时间戳
    current: u64,
}

impl TimestampOracle {
    /// 创建 TSO，初始时间戳为 0
    pub fn new() -> Self {
        Self { current: 0 }
    }

    /// 获取下一个时间戳（单调递增）
    pub fn get_ts(&mut self) -> u64 {
        self.current += 1;
        self.current
    }

    /// 当前时间戳（不递增）
    pub fn current(&self) -> u64 {
        self.current
    }
}

// =====================================================================
//  Mutation — 事务中的写操作
// =====================================================================

/// 事务中的单个写操作
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// 写入键值
    Put {
        /// 键
        key: Vec<u8>,
        /// 值
        value: Vec<u8>,
    },
    /// 删除键
    Delete {
        /// 键
        key: Vec<u8>,
    },
}

impl Mutation {
    /// 获取键引用
    pub fn key(&self) -> &[u8] {
        match self {
            Self::Put { key, .. } => key,
            Self::Delete { key } => key,
        }
    }

    /// 获取值（Delete 返回空切片）
    pub fn value(&self) -> &[u8] {
        match self {
            Self::Put { value, .. } => value,
            Self::Delete { .. } => &[],
        }
    }

    /// 锁类型字节
    pub fn lock_kind(&self) -> u8 {
        match self {
            Self::Put { .. } => LOCK_KIND_PUT,
            Self::Delete { .. } => LOCK_KIND_DELETE,
        }
    }

    /// 写记录类型字节
    pub fn write_kind(&self) -> u8 {
        match self {
            Self::Put { .. } => WRITE_KIND_PUT,
            Self::Delete { .. } => WRITE_KIND_DELETE,
        }
    }

    /// 创建 Put 变更
    pub fn put(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self::Put { key, value }
    }

    /// 创建 Delete 变更
    pub fn delete(key: Vec<u8>) -> Self {
        Self::Delete { key }
    }
}

// =====================================================================
//  LockInfo — 锁记录
// =====================================================================

/// 锁信息：prewrite 阶段写入，commit/rollback 阶段清除。
///
/// 锁记录存储在 `LOCK_PREFIX || key` 位置，包含指向事务主键的指针。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockInfo {
    /// 主键（事务的第一个写键，用于故障恢复时判断事务状态）
    pub primary_key: Vec<u8>,
    /// 事务开始时间戳
    pub start_ts: u64,
    /// 锁类型（Put / Delete）
    pub kind: u8,
    /// 待写入的值（Delete 时为空）
    pub value: Vec<u8>,
}

impl LockInfo {
    /// 编码为字节序列
    ///
    /// 格式：`[primary_key_len:u32 BE][primary_key][start_ts:u64 BE][kind:u8][value_len:u32 BE][value]`
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + self.primary_key.len() + 8 + 1 + 4 + self.value.len());
        buf.extend_from_slice(&(self.primary_key.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.primary_key);
        buf.extend_from_slice(&self.start_ts.to_be_bytes());
        buf.push(self.kind);
        buf.extend_from_slice(&(self.value.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.value);
        buf
    }

    /// 从字节序列解码
    ///
    /// # Errors
    /// 数据格式非法时返回 `TxnError::CorruptData`。
    pub fn decode(data: &[u8]) -> Result<Self, TxnError> {
        let mut pos = 0usize;
        if data.len() < 4 {
            return Err(TxnError::CorruptData);
        }
        let pk_len = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
        pos += 4;
        if data.len() < pos + pk_len + 8 + 1 + 4 {
            return Err(TxnError::CorruptData);
        }
        let primary_key = data[pos..pos + pk_len].to_vec();
        pos += pk_len;
        let start_ts = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let kind = data[pos];
        pos += 1;
        let val_len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if data.len() < pos + val_len {
            return Err(TxnError::CorruptData);
        }
        let value = data[pos..pos + val_len].to_vec();
        Ok(Self {
            primary_key,
            start_ts,
            kind,
            value,
        })
    }
}

// =====================================================================
//  WriteRecord — 写记录
// =====================================================================

/// 写记录：commit/rollback 阶段写入，记录该键在某 commit_ts 被某事务提交（或回滚）。
///
/// 写记录存储在 `WRITE_PREFIX || key || commit_ts` 位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteRecord {
    /// 事务开始时间戳（用于定位对应的 data 记录）
    pub start_ts: u64,
    /// 写记录类型（Put / Delete / Rollback）
    pub kind: u8,
}

impl WriteRecord {
    /// 编码为字节序列
    ///
    /// 格式：`[start_ts:u64 BE][kind:u8]`
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(9);
        buf.extend_from_slice(&self.start_ts.to_be_bytes());
        buf.push(self.kind);
        buf
    }

    /// 从字节序列解码
    ///
    /// # Errors
    /// 数据格式非法时返回 `TxnError::CorruptData`。
    pub fn decode(data: &[u8]) -> Result<Self, TxnError> {
        if data.len() < 9 {
            return Err(TxnError::CorruptData);
        }
        let start_ts = u64::from_be_bytes(data[0..8].try_into().unwrap());
        let kind = data[8];
        Ok(Self { start_ts, kind })
    }

    /// 从写键中提取 commit_ts
    ///
    /// 写键格式：`WRITE_PREFIX || key || commit_ts`，commit_ts 是最后 8 字节。
    pub fn extract_commit_ts(write_key: &[u8]) -> Option<u64> {
        if write_key.len() < 9 || write_key[0] != WRITE_PREFIX {
            return None;
        }
        let ts_bytes: [u8; 8] = write_key[write_key.len() - 8..].try_into().ok()?;
        Some(u64::from_be_bytes(ts_bytes))
    }

    /// 从写键中提取原始键
    ///
    /// 写键格式：`WRITE_PREFIX || key || commit_ts`，原始键是去掉前缀和后 8 字节后的部分。
    pub fn extract_key(write_key: &[u8]) -> Option<&[u8]> {
        if write_key.len() < 9 || write_key[0] != WRITE_PREFIX {
            return None;
        }
        Some(&write_key[1..write_key.len() - 8])
    }
}

// =====================================================================
//  TxnError — 事务错误
// =====================================================================

/// Percolator 事务错误
#[derive(Debug, Clone)]
pub enum TxnError {
    /// 键已被其他事务锁定
    KeyAlreadyLocked {
        /// 被锁定的键
        key: Vec<u8>,
        /// 持有锁的事务 start_ts
        holder_start_ts: u64,
    },
    /// 写冲突：另一事务在更高的 commit_ts 提交了同一键
    WriteConflict {
        /// 冲突键
        key: Vec<u8>,
    },
    /// 主键锁不存在（提交时）
    LockNotFound {
        /// 主键
        primary_key: Vec<u8>,
    },
    /// 读时遇到未提交的锁
    LockedOnRead {
        /// 被锁定的键
        key: Vec<u8>,
        /// 持有锁的事务 start_ts
        holder_start_ts: u64,
    },
    /// 路由错误（键无对应分片）
    RouteError(String),
    /// 底层 Raft 错误
    Raft(RaftError),
    /// 数据损坏（编码格式错误）
    CorruptData,
}

impl fmt::Display for TxnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyAlreadyLocked {
                key,
                holder_start_ts,
            } => write!(f, "key {:?} locked by txn@{}", key, holder_start_ts),
            Self::WriteConflict { key } => write!(f, "write conflict on key {:?}", key),
            Self::LockNotFound { primary_key } => {
                write!(f, "primary lock not found: {:?}", primary_key)
            }
            Self::LockedOnRead {
                key,
                holder_start_ts,
            } => write!(f, "read blocked by lock {:?}@{}", key, holder_start_ts),
            Self::RouteError(msg) => write!(f, "route error: {}", msg),
            Self::Raft(e) => write!(f, "raft error: {}", e),
            Self::CorruptData => write!(f, "corrupt data"),
        }
    }
}

impl std::error::Error for TxnError {}

impl From<RaftError> for TxnError {
    fn from(e: RaftError) -> Self {
        Self::Raft(e)
    }
}

// =====================================================================
//  键编码辅助
// =====================================================================

/// 构造数据键：`DATA_PREFIX || original_key || start_ts`
pub(crate) fn data_key(key: &[u8], start_ts: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + key.len() + 8);
    buf.push(DATA_PREFIX);
    buf.extend_from_slice(key);
    buf.extend_from_slice(&start_ts.to_be_bytes());
    buf
}

/// 构造锁键：`LOCK_PREFIX || original_key`
pub(crate) fn lock_key(key: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + key.len());
    buf.push(LOCK_PREFIX);
    buf.extend_from_slice(key);
    buf
}

/// 构造写键：`WRITE_PREFIX || original_key || commit_ts`
pub(crate) fn write_key(key: &[u8], commit_ts: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + key.len() + 8);
    buf.push(WRITE_PREFIX);
    buf.extend_from_slice(key);
    buf.extend_from_slice(&commit_ts.to_be_bytes());
    buf
}

/// 构造同一 original_key 的所有数据记录范围
///
/// 返回 (start, end)，覆盖 `DATA_PREFIX || key || 任意 start_ts`。
#[allow(dead_code)]
pub(crate) fn data_prefix_range(key: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut start = Vec::with_capacity(1 + key.len());
    start.push(DATA_PREFIX);
    start.extend_from_slice(key);
    // end = start 后追加 0xFF*8 + 0x00，保证覆盖所有 start_ts（8 字节 BE）
    let mut end = start.clone();
    end.extend_from_slice(&[0xFF; 8]);
    end.push(0x00);
    (start, end)
}

/// 构造同一 original_key 的所有写记录范围
pub(crate) fn write_prefix_range(key: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut start = Vec::with_capacity(1 + key.len());
    start.push(WRITE_PREFIX);
    start.extend_from_slice(key);
    let mut end = start.clone();
    end.extend_from_slice(&[0xFF; 8]);
    end.push(0x00);
    (start, end)
}

// =====================================================================
//  PercolatorClient — Percolator 事务客户端
// =====================================================================

/// Percolator 事务客户端
///
/// 封装 [`ShardCluster`]，提供跨分片的 ACID 事务能力。
///
/// # 协议流程
///
/// 1. **Begin**：从 TSO 获取 start_ts
/// 2. **Prewrite**（阶段一）：对每个写键
///    - 检查 write 记录（commit_ts > start_ts 则写冲突）
///    - 检查 lock 记录（存在则锁冲突）
///    - 写入 data 记录（带 start_ts 版本）
///    - 写入 lock 记录（含 primary_key 指针）
/// 3. **Commit**（阶段二）：
///    - 从 TSO 获取 commit_ts
///    - 写入 primary key 的 write 记录，删除 primary 的 lock
///    - 对每个 secondary，写入 write 记录，删除 lock
/// 4. **Rollback**：
///    - 删除所有 lock 记录
///    - 写入 ROLLBACK write 记录（防止延迟的 prewrite 成功）
pub struct PercolatorClient<'a> {
    /// 集群引用
    cluster: &'a mut ShardCluster,
    /// 时间戳服务引用
    tso: &'a mut TimestampOracle,
}

impl<'a> PercolatorClient<'a> {
    /// 创建客户端
    pub fn new(cluster: &'a mut ShardCluster, tso: &'a mut TimestampOracle) -> Self {
        Self { cluster, tso }
    }

    /// 获取开始时间戳
    pub fn begin(&mut self) -> u64 {
        self.tso.get_ts()
    }

    /// 路由键到分片
    fn route(&self, key: &[u8]) -> Result<ShardId, TxnError> {
        self.cluster
            .router
            .route(key)
            .map_err(|e| TxnError::RouteError(format!("{:?}", e)))
    }

    /// 读取键的最新已提交值（快照读）
    ///
    /// 在 read_ts 时刻，找该键最新的 commit_ts <= read_ts 的写记录，
    /// 若是 Put 则返回对应 data 值，若是 Delete 或 Rollback 则返回 None。
    ///
    /// # Errors
    /// - 若键被锁且 `lock.start_ts <= read_ts`，返回 `LockedOnRead`
    /// - 若键路由失败，返回 `RouteError`
    /// - 若数据损坏，返回 `CorruptData`
    pub fn get(&self, key: &[u8], read_ts: u64) -> Result<Option<Vec<u8>>, TxnError> {
        let shard_id = self.route(key)?;

        // 1. 检查锁：若 lock.start_ts <= read_ts，读被阻塞
        let lkey = lock_key(key);
        if let Some(lock_bytes) = self.cluster.get_from_leader(shard_id, &lkey) {
            let lock = LockInfo::decode(&lock_bytes)?;
            if lock.start_ts <= read_ts {
                return Err(TxnError::LockedOnRead {
                    key: key.to_vec(),
                    holder_start_ts: lock.start_ts,
                });
            }
        }

        // 2. 扫描写记录，找最新 commit_ts <= read_ts 的非 ROLLBACK 记录
        //    注意：必须扫描原始键所属分片（shard_id），而非按带前缀的存储键路由，
        //    因为前缀字节（0x03）会改变排序，导致路由到错误分片。
        let (start, end) = write_prefix_range(key);
        let range = KeyRange {
            start: Some(start),
            end: Some(end),
        };
        let writes = self.cluster.scan_shard(shard_id, &range);

        let mut latest: Option<(u64, WriteRecord)> = None; // (commit_ts, record)
        for (k, v) in writes {
            let Some(commit_ts) = WriteRecord::extract_commit_ts(&k) else {
                continue;
            };
            // 精确键匹配：避免 "acc1" 误匹配 "acc10"~"acc19" 等前缀冲突
            if WriteRecord::extract_key(&k) != Some(key) {
                continue;
            }
            if commit_ts > read_ts {
                continue;
            }
            let record = WriteRecord::decode(&v)?;
            if record.kind == WRITE_KIND_ROLLBACK {
                continue; // 跳过回滚记录
            }
            if latest.is_none() || commit_ts > latest.as_ref().unwrap().0 {
                latest = Some((commit_ts, record));
            }
        }

        match latest {
            None => Ok(None),
            Some((_, record)) => {
                if record.kind == WRITE_KIND_DELETE {
                    return Ok(None);
                }
                // 读取对应 start_ts 的 data 记录
                let dkey = data_key(key, record.start_ts);
                Ok(self.cluster.get_from_leader(shard_id, &dkey))
            }
        }
    }

    /// Prewrite 阶段：对单个键加锁 + 写入数据版本
    ///
    /// # 流程
    /// 1. 检查写冲突：扫描写记录，若有 commit_ts > start_ts 则冲突
    /// 2. 检查锁：若键已被锁则冲突
    /// 3. 写入 data 记录（`DATA_PREFIX || key || start_ts`）
    /// 4. 写入 lock 记录（`LOCK_PREFIX || key`）
    ///
    /// # Errors
    /// - 键已被锁：`KeyAlreadyLocked`
    /// - 写冲突：`WriteConflict`
    /// - 路由或 Raft 错误
    pub fn prewrite(
        &mut self,
        mutation: &Mutation,
        primary_key: &[u8],
        start_ts: u64,
    ) -> Result<(), TxnError> {
        let key = mutation.key();
        let shard_id = self.route(key)?;

        // 1. 检查写冲突：commit_ts > start_ts 的写记录表示冲突
        //    扫描原始键所属分片，避免前缀字节导致路由错误。
        let (start, end) = write_prefix_range(key);
        let range = KeyRange {
            start: Some(start),
            end: Some(end),
        };
        let writes = self.cluster.scan_shard(shard_id, &range);
        for (k, _) in &writes {
            if let Some(commit_ts) = WriteRecord::extract_commit_ts(k) {
                // 精确键匹配：避免前缀冲突导致误判写冲突
                if WriteRecord::extract_key(k) != Some(key) {
                    continue;
                }
                if commit_ts > start_ts {
                    return Err(TxnError::WriteConflict { key: key.to_vec() });
                }
            }
        }

        // 2. 检查锁
        let lkey = lock_key(key);
        if let Some(existing) = self.cluster.get_from_leader(shard_id, &lkey) {
            let lock = LockInfo::decode(&existing)?;
            return Err(TxnError::KeyAlreadyLocked {
                key: key.to_vec(),
                holder_start_ts: lock.start_ts,
            });
        }

        // 3. 写入 data 记录
        let dkey = data_key(key, start_ts);
        self.cluster
            .put(shard_id, dkey, mutation.value().to_vec())?;
        self.cluster.run_for(500);

        // 4. 写入 lock 记录
        let lock = LockInfo {
            primary_key: primary_key.to_vec(),
            start_ts,
            kind: mutation.lock_kind(),
            value: mutation.value().to_vec(),
        };
        self.cluster.put(shard_id, lkey, lock.encode())?;
        self.cluster.run_for(500);

        Ok(())
    }

    /// Prewrite 所有写操作（事务的第一个写键作为 primary）
    ///
    /// 便利方法：对 mutations 中的所有键执行 prewrite。
    /// 第一个键作为 primary_key，其余为 secondary。
    ///
    /// # Errors
    /// 任一键 prewrite 失败时返回（已 prewrite 的键不会自动回滚）。
    pub fn prewrite_all(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<(), TxnError> {
        if mutations.is_empty() {
            return Ok(());
        }
        let primary_key = mutations[0].key().to_vec();
        for m in mutations {
            self.prewrite(m, &primary_key, start_ts)?;
        }
        Ok(())
    }

    /// Commit 阶段：提交事务
    ///
    /// # 流程
    /// 1. 获取 commit_ts
    /// 2. 检查 primary 锁是否存在
    /// 3. 写入 primary 的 write 记录，删除 primary 的 lock
    /// 4. 对每个 secondary，写入 write 记录，删除 lock
    ///
    /// # Errors
    /// - Primary 锁不存在：`LockNotFound`
    /// - 路由或 Raft 错误
    pub fn commit(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<u64, TxnError> {
        if mutations.is_empty() {
            return Ok(self.tso.get_ts());
        }
        let commit_ts = self.tso.get_ts();

        // 提交 primary
        let primary = &mutations[0];
        let primary_key = primary.key();
        let primary_shard = self.route(primary_key)?;

        // 检查 primary 锁是否存在且 start_ts 匹配
        let primary_lkey = lock_key(primary_key);
        let primary_lock_bytes = self
            .cluster
            .get_from_leader(primary_shard, &primary_lkey)
            .ok_or_else(|| TxnError::LockNotFound {
                primary_key: primary_key.to_vec(),
            })?;
        let primary_lock = LockInfo::decode(&primary_lock_bytes)?;
        if primary_lock.start_ts != start_ts {
            return Err(TxnError::LockNotFound {
                primary_key: primary_key.to_vec(),
            });
        }

        // 写入 primary write 记录
        let primary_wkey = write_key(primary_key, commit_ts);
        let primary_write = WriteRecord {
            start_ts,
            kind: primary.write_kind(),
        };
        self.cluster
            .put(primary_shard, primary_wkey, primary_write.encode())?;
        self.cluster.run_for(500);

        // 删除 primary lock
        self.cluster.delete(primary_shard, primary_lkey)?;
        self.cluster.run_for(500);

        // 提交 secondaries
        for m in &mutations[1..] {
            let key = m.key();
            let shard = self.route(key)?;
            let wkey = write_key(key, commit_ts);
            let wrecord = WriteRecord {
                start_ts,
                kind: m.write_kind(),
            };
            self.cluster.put(shard, wkey, wrecord.encode())?;
            // 删除 secondary lock
            let lkey = lock_key(key);
            self.cluster.delete(shard, lkey)?;
        }
        self.cluster.run_for(500);

        Ok(commit_ts)
    }

    /// Rollback：回滚事务
    ///
    /// # 流程
    /// 1. 对每个键：删除 lock 记录
    /// 2. 对每个键：写入 ROLLBACK write 记录（防止延迟的 prewrite 成功后误读）
    /// 3. 对每个键：删除 data 记录（清理）
    ///
    /// # Errors
    /// 路由或 Raft 错误时返回。
    pub fn rollback(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<(), TxnError> {
        let rollback_ts = self.tso.get_ts(); // 用于 ROLLBACK 记录的 commit_ts 位置

        for m in mutations {
            let key = m.key();
            let shard = self.route(key)?;

            // 1. 删除 lock 记录
            let lkey = lock_key(key);
            self.cluster.delete(shard, lkey)?;

            // 2. 写入 ROLLBACK write 记录
            let wkey = write_key(key, rollback_ts);
            let wrecord = WriteRecord {
                start_ts,
                kind: WRITE_KIND_ROLLBACK,
            };
            self.cluster.put(shard, wkey, wrecord.encode())?;

            // 3. 删除 data 记录（清理）
            let dkey = data_key(key, start_ts);
            self.cluster.delete(shard, dkey)?;
        }
        self.cluster.run_for(500);

        Ok(())
    }

    /// 解决键上的残留锁（模拟故障恢复）
    ///
    /// 当读操作遇到锁时，调用此方法判断持有锁的事务是否已提交：
    /// - 检查 primary key 的写记录：
    ///   - 若有 COMMIT 记录 → 事务已提交，前推：写 secondary 的 COMMIT 记录，删除 lock
    ///   - 若有 ROLLBACK 记录 → 事务已回滚，回滚：写 ROLLBACK 记录，删除 lock
    ///   - 若 primary 也有锁 → 事务未完成，回滚
    ///
    /// # Errors
    /// 路由或 Raft 错误时返回。
    pub fn resolve_lock(&mut self, key: &[u8]) -> Result<ResolveResult, TxnError> {
        let shard_id = self.route(key)?;
        let lkey = lock_key(key);

        // 读取锁
        let lock_bytes = match self.cluster.get_from_leader(shard_id, &lkey) {
            Some(b) => b,
            None => return Ok(ResolveResult::NoLock),
        };
        let lock = LockInfo::decode(&lock_bytes)?;

        // 检查 primary key 的写记录
        let primary_shard = self.route(&lock.primary_key)?;

        // 扫描 primary 的写记录（写记录是事务状态的最终判据）。
        // 注意：即使 primary 锁仍存在，也可能已有 COMMIT 写记录（commit 阶段
        // 部分失败：写记录已写入但锁删除失败）。此时事务已提交，必须前推而非回滚，
        // 否则 ROLLBACK 记录会覆盖 COMMIT 记录导致数据损坏。
        let (start, end) = write_prefix_range(&lock.primary_key);
        let range = KeyRange {
            start: Some(start),
            end: Some(end),
        };
        let primary_writes = self.cluster.scan_shard(primary_shard, &range);

        // 找到 start_ts 匹配的写记录
        for (k, v) in &primary_writes {
            let record = WriteRecord::decode(v)?;
            // 精确键匹配：避免前缀冲突导致误判事务状态
            if WriteRecord::extract_key(k) != Some(&lock.primary_key) {
                continue;
            }
            if record.start_ts != lock.start_ts {
                continue;
            }
            let commit_ts = WriteRecord::extract_commit_ts(k).unwrap_or(0);
            match record.kind {
                WRITE_KIND_PUT | WRITE_KIND_DELETE => {
                    // 事务已提交，前推 secondary
                    self.commit_single(key, &lock, shard_id, commit_ts, record.kind)?;
                    return Ok(ResolveResult::Committed {
                        start_ts: lock.start_ts,
                        commit_ts,
                    });
                }
                WRITE_KIND_ROLLBACK => {
                    // 事务已回滚，回滚 secondary
                    self.rollback_single(key, &lock, shard_id)?;
                    return Ok(ResolveResult::RolledBack {
                        start_ts: lock.start_ts,
                    });
                }
                _ => {}
            }
        }

        // 未找到匹配的写记录，回滚
        self.rollback_single(key, &lock, shard_id)?;
        Ok(ResolveResult::RolledBack {
            start_ts: lock.start_ts,
        })
    }

    /// 内部：前推提交单个键
    fn commit_single(
        &mut self,
        key: &[u8],
        lock: &LockInfo,
        shard_id: ShardId,
        commit_ts: u64,
        kind: u8,
    ) -> Result<(), TxnError> {
        let wkey = write_key(key, commit_ts);
        let wrecord = WriteRecord {
            start_ts: lock.start_ts,
            kind,
        };
        self.cluster.put(shard_id, wkey, wrecord.encode())?;
        let lkey = lock_key(key);
        self.cluster.delete(shard_id, lkey)?;
        self.cluster.run_for(500);
        Ok(())
    }

    /// 内部：回滚单个键
    fn rollback_single(
        &mut self,
        key: &[u8],
        lock: &LockInfo,
        shard_id: ShardId,
    ) -> Result<(), TxnError> {
        let rollback_ts = self.tso.get_ts();
        let wkey = write_key(key, rollback_ts);
        let wrecord = WriteRecord {
            start_ts: lock.start_ts,
            kind: WRITE_KIND_ROLLBACK,
        };
        self.cluster.put(shard_id, wkey, wrecord.encode())?;
        let lkey = lock_key(key);
        self.cluster.delete(shard_id, lkey)?;
        let dkey = data_key(key, lock.start_ts);
        self.cluster.delete(shard_id, dkey)?;
        self.cluster.run_for(500);
        Ok(())
    }
}

// =====================================================================
//  ResolveResult — 锁解决结果
// =====================================================================

/// `resolve_lock` 的返回结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveResult {
    /// 键无锁
    NoLock,
    /// 事务已提交（前推）
    Committed {
        /// 事务 start_ts
        start_ts: u64,
        /// 提交 commit_ts
        commit_ts: u64,
    },
    /// 事务已回滚
    RolledBack {
        /// 事务 start_ts
        start_ts: u64,
    },
}

// =====================================================================
//  测试模块
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::{KeyRange, Shard, ShardCluster};

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    /// 创建 3 节点、2 分片集群：
    /// - shard 1: (-∞, "m")  节点 1,2,3
    /// - shard 2: ["m", +∞)  节点 1,2,3
    fn make_cluster() -> ShardCluster {
        let nodes = vec![1u64, 2, 3];
        let shards = vec![
            Shard::new(
                1,
                KeyRange {
                    start: None,
                    end: Some(b"m".to_vec()),
                },
                vec![1, 2, 3],
            ),
            Shard::new(
                2,
                KeyRange {
                    start: Some(b"m".to_vec()),
                    end: None,
                },
                vec![1, 2, 3],
            ),
        ];
        let mut cluster = ShardCluster::new(&nodes, shards, 42);
        cluster.run_for(1000); // 等待选举完成
        cluster
    }

    // -----------------------------------------------------------------
    //  1. TimestampOracle 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_tso_monotonic() {
        let mut tso = TimestampOracle::new();
        let ts1 = tso.get_ts();
        let ts2 = tso.get_ts();
        let ts3 = tso.get_ts();
        assert!(ts1 < ts2);
        assert!(ts2 < ts3);
        assert_eq!(tso.current(), ts3);
    }

    #[test]
    fn test_tso_starts_from_one() {
        let mut tso = TimestampOracle::new();
        assert_eq!(tso.get_ts(), 1);
        assert_eq!(tso.get_ts(), 2);
    }

    // -----------------------------------------------------------------
    //  2. Mutation 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_mutation_put_helpers() {
        let m = Mutation::put(b"k1".to_vec(), b"v1".to_vec());
        assert_eq!(m.key(), b"k1");
        assert_eq!(m.value(), b"v1");
        assert_eq!(m.lock_kind(), LOCK_KIND_PUT);
        assert_eq!(m.write_kind(), WRITE_KIND_PUT);
    }

    #[test]
    fn test_mutation_delete_helpers() {
        let m = Mutation::delete(b"k1".to_vec());
        assert_eq!(m.key(), b"k1");
        assert!(m.value().is_empty());
        assert_eq!(m.lock_kind(), LOCK_KIND_DELETE);
        assert_eq!(m.write_kind(), WRITE_KIND_DELETE);
    }

    // -----------------------------------------------------------------
    //  3. LockInfo 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn test_lock_info_encode_decode() {
        let lock = LockInfo {
            primary_key: b"pk".to_vec(),
            start_ts: 42,
            kind: LOCK_KIND_PUT,
            value: b"hello".to_vec(),
        };
        let encoded = lock.encode();
        let decoded = LockInfo::decode(&encoded).unwrap();
        assert_eq!(lock, decoded);
    }

    #[test]
    fn test_lock_info_empty_value() {
        let lock = LockInfo {
            primary_key: b"pk".to_vec(),
            start_ts: 1,
            kind: LOCK_KIND_DELETE,
            value: Vec::new(),
        };
        let encoded = lock.encode();
        let decoded = LockInfo::decode(&encoded).unwrap();
        assert_eq!(lock, decoded);
    }

    #[test]
    fn test_lock_info_decode_corrupt() {
        assert!(LockInfo::decode(&[]).is_err());
        assert!(LockInfo::decode(&[0x00, 0x00, 0x00, 0x05]).is_err()); // 声称 pk_len=5 但数据不够
    }

    // -----------------------------------------------------------------
    //  4. WriteRecord 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn test_write_record_encode_decode() {
        let wr = WriteRecord {
            start_ts: 100,
            kind: WRITE_KIND_PUT,
        };
        let encoded = wr.encode();
        assert_eq!(encoded.len(), 9);
        let decoded = WriteRecord::decode(&encoded).unwrap();
        assert_eq!(wr, decoded);
    }

    #[test]
    fn test_write_record_extract_commit_ts() {
        let wk = write_key(b"mykey", 12345);
        assert_eq!(WriteRecord::extract_commit_ts(&wk), Some(12345));
    }

    #[test]
    fn test_write_record_extract_commit_ts_invalid() {
        assert_eq!(WriteRecord::extract_commit_ts(b"short"), None);
        assert_eq!(WriteRecord::extract_commit_ts(&[DATA_PREFIX, 0, 0]), None);
    }

    #[test]
    fn test_write_record_decode_corrupt() {
        assert!(WriteRecord::decode(&[0, 1, 2]).is_err());
    }

    // -----------------------------------------------------------------
    //  5. 键编码测试
    // -----------------------------------------------------------------

    #[test]
    fn test_data_key_format() {
        let dk = data_key(b"abc", 0x0102);
        assert_eq!(dk[0], DATA_PREFIX);
        assert_eq!(&dk[1..4], b"abc");
        assert_eq!(&dk[4..], &0x0102u64.to_be_bytes());
    }

    #[test]
    fn test_lock_key_format() {
        let lk = lock_key(b"xyz");
        assert_eq!(lk[0], LOCK_PREFIX);
        assert_eq!(&lk[1..], b"xyz");
    }

    #[test]
    fn test_write_key_format() {
        let wk = write_key(b"k", 99);
        assert_eq!(wk[0], WRITE_PREFIX);
        assert_eq!(&wk[1..2], b"k");
        assert_eq!(WriteRecord::extract_commit_ts(&wk), Some(99));
    }

    #[test]
    fn test_data_prefix_range_covers_all_ts() {
        let (start, end) = data_prefix_range(b"key");
        // start_ts = 0 的键应 >= start
        let dk_min = data_key(b"key", 0);
        assert!(dk_min >= start);
        assert!(dk_min < end);
        // start_ts = u64::MAX 的键应 < end
        let dk_max = data_key(b"key", u64::MAX);
        assert!(dk_max >= start);
        assert!(dk_max < end);
    }

    #[test]
    fn test_write_prefix_range_covers_all_ts() {
        let (start, end) = write_prefix_range(b"key");
        let wk_min = write_key(b"key", 0);
        let wk_max = write_key(b"key", u64::MAX);
        assert!(wk_min >= start);
        assert!(wk_min < end);
        assert!(wk_max >= start);
        assert!(wk_max < end);
    }

    // -----------------------------------------------------------------
    //  6. TxnError 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_txn_error_display() {
        let e = TxnError::WriteConflict { key: b"k".to_vec() };
        assert!(format!("{}", e).contains("write conflict"));
    }

    #[test]
    fn test_txn_error_from_raft() {
        let raft_err = RaftError::ConfigError("test".into());
        let txn_err: TxnError = raft_err.into();
        assert!(matches!(txn_err, TxnError::Raft(_)));
    }

    // -----------------------------------------------------------------
    //  7. Percolator 基本事务（单分片单键）
    // -----------------------------------------------------------------

    #[test]
    fn test_single_key_put_and_get() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 事务 T1：写入 key "a" = "v1"
        let mut client = PercolatorClient::new(&mut cluster, &mut tso);
        let start_ts = client.begin();
        client
            .prewrite(
                &Mutation::put(b"a".to_vec(), b"v1".to_vec()),
                b"a",
                start_ts,
            )
            .unwrap();
        let commit_ts = client
            .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], start_ts)
            .unwrap();

        // 读回验证
        let read_ts = client.begin();
        let val = client.get(b"a", read_ts).unwrap();
        assert_eq!(val, Some(b"v1".to_vec()));
        assert!(commit_ts < read_ts);
    }

    #[test]
    fn test_single_key_overwrite() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: a = v1
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], ts)
                .unwrap();
        }

        // T2: a = v2
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v2".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v2".to_vec())], ts)
                .unwrap();
        }

        // 读最新值
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            assert_eq!(client.get(b"a", ts).unwrap(), Some(b"v2".to_vec()));
        }
    }

    #[test]
    fn test_single_key_delete() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: 写入 a = v1
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], ts)
                .unwrap();
        }

        // T2: 删除 a
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::delete(b"a".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::delete(b"a".to_vec())], ts)
                .unwrap();
        }

        // 读应返回 None
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            assert_eq!(client.get(b"a", ts).unwrap(), None);
        }
    }

    // -----------------------------------------------------------------
    //  8. 跨分片事务（核心验证）
    // -----------------------------------------------------------------

    #[test]
    fn test_cross_shard_transfer_total_conservation() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化：alice = 100（shard 1），bob = 50（shard 2）
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(b"alice".to_vec(), b"100".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();
            client
                .commit(&[Mutation::put(b"alice".to_vec(), b"100".to_vec())], ts)
                .unwrap();
        }
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"bob".to_vec(), b"50".to_vec()), b"bob", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"bob".to_vec(), b"50".to_vec())], ts)
                .unwrap();
        }

        // 转账：alice - 30，bob + 30
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let mutations = vec![
                Mutation::put(b"alice".to_vec(), b"70".to_vec()),
                Mutation::put(b"bob".to_vec(), b"80".to_vec()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            client.commit(&mutations, ts).unwrap();
        }

        // 验证总额守恒：70 + 80 = 150 = 100 + 50
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let alice: u64 = std::str::from_utf8(&client.get(b"alice", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let bob: u64 = std::str::from_utf8(&client.get(b"bob", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(alice, 70);
            assert_eq!(bob, 80);
            assert_eq!(alice + bob, 150); // 总额守恒
        }
    }

    #[test]
    fn test_cross_shard_transfer_rollback_total_conservation() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化：alice = 100，bob = 50
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(b"alice".to_vec(), b"100".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();
            client
                .commit(&[Mutation::put(b"alice".to_vec(), b"100".to_vec())], ts)
                .unwrap();
        }
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"bob".to_vec(), b"50".to_vec()), b"bob", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"bob".to_vec(), b"50".to_vec())], ts)
                .unwrap();
        }

        // 转账中途崩溃：prewrite alice 成功，prewrite bob 前崩溃
        let crashed_start_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(b"alice".to_vec(), b"70".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();
            // "崩溃"：不继续 prewrite bob，不 commit
            ts
        };

        // 回滚崩溃的事务
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            client
                .rollback(
                    &[Mutation::put(b"alice".to_vec(), b"70".to_vec())],
                    crashed_start_ts,
                )
                .unwrap();
        }

        // 验证总额守恒：alice=100, bob=50, total=150
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let alice: u64 = std::str::from_utf8(&client.get(b"alice", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let bob: u64 = std::str::from_utf8(&client.get(b"bob", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(alice, 100);
            assert_eq!(bob, 50);
            assert_eq!(alice + bob, 150);
        }
    }

    #[test]
    fn test_cross_shard_transfer_both_prewrite_then_rollback() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(b"alice".to_vec(), b"100".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();
            client
                .commit(&[Mutation::put(b"alice".to_vec(), b"100".to_vec())], ts)
                .unwrap();
        }
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"bob".to_vec(), b"50".to_vec()), b"bob", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"bob".to_vec(), b"50".to_vec())], ts)
                .unwrap();
        }

        // prewrite alice + bob 成功，但 commit 前崩溃
        let crashed_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let mutations = vec![
                Mutation::put(b"alice".to_vec(), b"70".to_vec()),
                Mutation::put(b"bob".to_vec(), b"80".to_vec()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            // "崩溃"：不 commit
            ts
        };

        // 回滚
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            client
                .rollback(
                    &[
                        Mutation::put(b"alice".to_vec(), b"70".to_vec()),
                        Mutation::put(b"bob".to_vec(), b"80".to_vec()),
                    ],
                    crashed_ts,
                )
                .unwrap();
        }

        // 验证
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let alice: u64 = std::str::from_utf8(&client.get(b"alice", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let bob: u64 = std::str::from_utf8(&client.get(b"bob", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(alice, 100);
            assert_eq!(bob, 50);
            assert_eq!(alice + bob, 150);
        }
    }

    // -----------------------------------------------------------------
    //  9. 冲突检测测试
    // -----------------------------------------------------------------

    #[test]
    fn test_write_conflict_detected() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: a = v1，提交
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], ts)
                .unwrap();
        }

        // T2: start_ts = 1（在 T1 提交前），尝试写 a → 应写冲突
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            // 手动设置较低的 start_ts
            let start_ts = 1u64;
            let result = client.prewrite(
                &Mutation::put(b"a".to_vec(), b"v2".to_vec()),
                b"a",
                start_ts,
            );
            assert!(matches!(result, Err(TxnError::WriteConflict { .. })));
        }
    }

    #[test]
    fn test_lock_conflict_detected() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: prewrite a（加锁，不提交）
        let t1_start_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            ts
        };

        // T2: 尝试 prewrite 同一 key → 应锁冲突
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let result = client.prewrite(&Mutation::put(b"a".to_vec(), b"v2".to_vec()), b"a", ts);
            assert!(matches!(
                result,
                Err(TxnError::KeyAlreadyLocked {
                    holder_start_ts,
                    ..
                }) if holder_start_ts == t1_start_ts
            ));
        }
    }

    #[test]
    fn test_read_blocked_by_lock() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: prewrite a = v1（加锁，不提交）
        let t1_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            ts
        };

        // T2: 尝试读 a → 应被锁阻塞
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let read_ts = client.begin();
            let result = client.get(b"a", read_ts);
            assert!(matches!(
                result,
                Err(TxnError::LockedOnRead {
                    holder_start_ts,
                    ..
                }) if holder_start_ts == t1_ts
            ));
        }
    }

    // -----------------------------------------------------------------
    //  10. 故障恢复 — resolve_lock 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_resolve_lock_rolls_back_uncommitted() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化 a = v1
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], ts)
                .unwrap();
        }

        // T1: prewrite a = v2（primary=a），不提交 → 模拟崩溃
        let t1_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v2".to_vec()), b"a", ts)
                .unwrap();
            ts
        };

        // 新客户端解决残留锁
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let result = client.resolve_lock(b"a").unwrap();
            assert_eq!(result, ResolveResult::RolledBack { start_ts: t1_ts });
        }

        // 读应返回 v1（原始值）
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            assert_eq!(client.get(b"a", ts).unwrap(), Some(b"v1".to_vec()));
        }
    }

    #[test]
    fn test_resolve_lock_committed_primary_pushes_secondary() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(b"alice".to_vec(), b"100".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();
            client
                .commit(&[Mutation::put(b"alice".to_vec(), b"100".to_vec())], ts)
                .unwrap();
        }
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"bob".to_vec(), b"50".to_vec()), b"bob", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"bob".to_vec(), b"50".to_vec())], ts)
                .unwrap();
        }

        // T1: prewrite alice + bob，primary=alice
        // 然后手动提交 primary（alice），但不提交 secondary（bob）
        let (t1_ts, t1_commit_ts) = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(b"alice".to_vec(), b"70".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();
            client
                .prewrite(
                    &Mutation::put(b"bob".to_vec(), b"80".to_vec()),
                    b"alice",
                    ts,
                )
                .unwrap();

            // 手动提交 primary（模拟 commit 到一半崩溃）
            let commit_ts = client.tso.get_ts();
            let primary_shard = client.route(b"alice").unwrap();
            let wkey = write_key(b"alice", commit_ts);
            let wrecord = WriteRecord {
                start_ts: ts,
                kind: WRITE_KIND_PUT,
            };
            client
                .cluster
                .put(primary_shard, wkey, wrecord.encode())
                .unwrap();
            client.cluster.run_for(500);
            // 删除 primary lock
            let lkey = lock_key(b"alice");
            client.cluster.delete(primary_shard, lkey).unwrap();
            client.cluster.run_for(500);
            // secondary（bob）的 lock 仍在
            (ts, commit_ts)
        };

        // 解决 bob 的残留锁 → 应前推（COMMITTED）
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let result = client.resolve_lock(b"bob").unwrap();
            assert_eq!(
                result,
                ResolveResult::Committed {
                    start_ts: t1_ts,
                    commit_ts: t1_commit_ts,
                }
            );
        }

        // 验证：alice=70, bob=80, total=150
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let alice: u64 = std::str::from_utf8(&client.get(b"alice", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let bob: u64 = std::str::from_utf8(&client.get(b"bob", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(alice, 70);
            assert_eq!(bob, 80);
            assert_eq!(alice + bob, 150);
        }
    }

    #[test]
    fn test_resolve_lock_no_lock() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();
        let mut client = PercolatorClient::new(&mut cluster, &mut tso);
        let result = client.resolve_lock(b"nonexistent").unwrap();
        assert_eq!(result, ResolveResult::NoLock);
    }

    // -----------------------------------------------------------------
    //  11. 快照读隔离测试
    // -----------------------------------------------------------------

    #[test]
    fn test_snapshot_read_isolation() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: a = v1
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], ts)
                .unwrap();
        }

        // T2: 在 T3 写入前获取读时间戳
        let t2_read_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            client.begin()
        };

        // T3: a = v2
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v2".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v2".to_vec())], ts)
                .unwrap();
        }

        // T2 读应看到 v1（快照隔离）
        {
            let client = PercolatorClient::new(&mut cluster, &mut tso);
            let val = client.get(b"a", t2_read_ts).unwrap();
            assert_eq!(val, Some(b"v1".to_vec()));
        }
    }

    #[test]
    fn test_serializable_no_intermediate_state() {
        // 验证：T1 写 x=1, T2 写 x=2 → 串行化 → 最终 x=1 或 x=2，无中间态
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: x = 1
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"x".to_vec(), b"1".to_vec()), b"x", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"x".to_vec(), b"1".to_vec())], ts)
                .unwrap();
        }

        // T2: x = 2
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"x".to_vec(), b"2".to_vec()), b"x", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"x".to_vec(), b"2".to_vec())], ts)
                .unwrap();
        }

        // 最终值应为 1 或 2，不是中间态
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let val = client.get(b"x", ts).unwrap();
            let val_bytes = val.unwrap();
            let val_str = std::str::from_utf8(&val_bytes).unwrap();
            assert!(
                val_str == "1" || val_str == "2",
                "expected 1 or 2, got {}",
                val_str
            );
            // 最终应为 2（最后提交的）
            assert_eq!(val_str, "2");
        }
    }

    // -----------------------------------------------------------------
    //  12. 多事务串行化测试
    // -----------------------------------------------------------------

    #[test]
    fn test_concurrent_transactions_serializable() {
        // 两个事务并发写同一键，最终结果应为其中一个的写入值
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化 a = 0
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"0".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"0".to_vec())], ts)
                .unwrap();
        }

        // T1: prewrite a = 1
        let t1_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"1".to_vec()), b"a", ts)
                .unwrap();
            ts
        };

        // T2: 尝试 prewrite a = 2 → 应锁冲突
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let result = client.prewrite(&Mutation::put(b"a".to_vec(), b"2".to_vec()), b"a", ts);
            assert!(matches!(result, Err(TxnError::KeyAlreadyLocked { .. })));
        }

        // T1 提交
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"1".to_vec())], t1_ts)
                .unwrap();
        }

        // 最终值应为 1
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            assert_eq!(client.get(b"a", ts).unwrap(), Some(b"1".to_vec()));
        }
    }

    // -----------------------------------------------------------------
    //  13. 多分片多键事务测试
    // -----------------------------------------------------------------

    #[test]
    fn test_multi_shard_three_key_transaction() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化：a（shard1）=1, n（shard2）=2, z（shard2）=3
        for (k, v) in [("a", "1"), ("n", "2"), ("z", "3")] {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(k.as_bytes().to_vec(), v.as_bytes().to_vec()),
                    k.as_bytes(),
                    ts,
                )
                .unwrap();
            client
                .commit(
                    &[Mutation::put(k.as_bytes().to_vec(), v.as_bytes().to_vec())],
                    ts,
                )
                .unwrap();
        }

        // 事务：a=10, n=20, z=30（跨两个分片）
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let mutations = vec![
                Mutation::put(b"a".to_vec(), b"10".to_vec()),
                Mutation::put(b"n".to_vec(), b"20".to_vec()),
                Mutation::put(b"z".to_vec(), b"30".to_vec()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            client.commit(&mutations, ts).unwrap();
        }

        // 验证
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            assert_eq!(client.get(b"a", ts).unwrap(), Some(b"10".to_vec()));
            assert_eq!(client.get(b"n", ts).unwrap(), Some(b"20".to_vec()));
            assert_eq!(client.get(b"z", ts).unwrap(), Some(b"30".to_vec()));
        }
    }

    #[test]
    fn test_multi_shard_rollback_all_keys() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化
        for (k, v) in [("a", "1"), ("n", "2"), ("z", "3")] {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(k.as_bytes().to_vec(), v.as_bytes().to_vec()),
                    k.as_bytes(),
                    ts,
                )
                .unwrap();
            client
                .commit(
                    &[Mutation::put(k.as_bytes().to_vec(), v.as_bytes().to_vec())],
                    ts,
                )
                .unwrap();
        }

        // prewrite 三键，然后回滚
        let crashed_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let mutations = vec![
                Mutation::put(b"a".to_vec(), b"10".to_vec()),
                Mutation::put(b"n".to_vec(), b"20".to_vec()),
                Mutation::put(b"z".to_vec(), b"30".to_vec()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            ts
        };

        // 回滚
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            client
                .rollback(
                    &[
                        Mutation::put(b"a".to_vec(), b"10".to_vec()),
                        Mutation::put(b"n".to_vec(), b"20".to_vec()),
                        Mutation::put(b"z".to_vec(), b"30".to_vec()),
                    ],
                    crashed_ts,
                )
                .unwrap();
        }

        // 验证原始值
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            assert_eq!(client.get(b"a", ts).unwrap(), Some(b"1".to_vec()));
            assert_eq!(client.get(b"n", ts).unwrap(), Some(b"2".to_vec()));
            assert_eq!(client.get(b"z", ts).unwrap(), Some(b"3".to_vec()));
        }
    }

    // -----------------------------------------------------------------
    //  14. 空事务测试
    // -----------------------------------------------------------------

    #[test]
    fn test_empty_transaction_commit() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();
        let mut client = PercolatorClient::new(&mut cluster, &mut tso);
        let ts = client.begin();
        let result = client.commit(&[], ts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_prewrite_all() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();
        let mut client = PercolatorClient::new(&mut cluster, &mut tso);
        let ts = client.begin();
        assert!(client.prewrite_all(&[], ts).is_ok());
    }

    // -----------------------------------------------------------------
    //  15. 读取不存在的键
    // -----------------------------------------------------------------

    #[test]
    fn test_get_nonexistent_key() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();
        let mut client = PercolatorClient::new(&mut cluster, &mut tso);
        let ts = client.begin();
        assert_eq!(client.get(b"nonexistent", ts).unwrap(), None);
    }

    // -----------------------------------------------------------------
    //  16. 银行场景完整测试
    // -----------------------------------------------------------------

    #[test]
    fn test_bank_scenario_multiple_transfers() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // 初始化三个账户
        for (k, v) in [("alice", "1000"), ("bob", "500"), ("carol", "300")] {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(
                    &Mutation::put(k.as_bytes().to_vec(), v.as_bytes().to_vec()),
                    k.as_bytes(),
                    ts,
                )
                .unwrap();
            client
                .commit(
                    &[Mutation::put(k.as_bytes().to_vec(), v.as_bytes().to_vec())],
                    ts,
                )
                .unwrap();
        }

        let initial_total: u64 = 1000 + 500 + 300;

        // 执行 5 笔转账
        let transfers: [(&[u8], &[u8], u64); 5] = [
            (b"alice", b"bob", 100),
            (b"bob", b"carol", 50),
            (b"carol", b"alice", 25),
            (b"alice", b"carol", 200),
            (b"bob", b"alice", 75),
        ];

        for (from, to, amount) in transfers {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();

            // 读取当前余额
            let from_bal: u64 = std::str::from_utf8(&client.get(from, ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let to_bal: u64 = std::str::from_utf8(&client.get(to, ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();

            let mutations = vec![
                Mutation::put(from.to_vec(), (from_bal - amount).to_string().into_bytes()),
                Mutation::put(to.to_vec(), (to_bal + amount).to_string().into_bytes()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            client.commit(&mutations, ts).unwrap();
        }

        // 验证总额守恒
        {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            let alice: u64 = std::str::from_utf8(&client.get(b"alice", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let bob: u64 = std::str::from_utf8(&client.get(b"bob", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            let carol: u64 = std::str::from_utf8(&client.get(b"carol", ts).unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(alice + bob + carol, initial_total);
        }
    }

    // -----------------------------------------------------------------
    //  17. commit_ts 排序测试
    // -----------------------------------------------------------------

    #[test]
    fn test_commit_ts_ordering() {
        let mut cluster = make_cluster();
        let mut tso = TimestampOracle::new();

        // T1: a = v1
        let t1_commit_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v1".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v1".to_vec())], ts)
                .unwrap()
        };

        // T2: a = v2
        let t2_commit_ts = {
            let mut client = PercolatorClient::new(&mut cluster, &mut tso);
            let ts = client.begin();
            client
                .prewrite(&Mutation::put(b"a".to_vec(), b"v2".to_vec()), b"a", ts)
                .unwrap();
            client
                .commit(&[Mutation::put(b"a".to_vec(), b"v2".to_vec())], ts)
                .unwrap()
        };

        assert!(t1_commit_ts < t2_commit_ts);

        // 在 t1_commit_ts 时读应看到 v1
        {
            let client = PercolatorClient::new(&mut cluster, &mut tso);
            assert_eq!(
                client.get(b"a", t1_commit_ts).unwrap(),
                Some(b"v1".to_vec())
            );
        }

        // 在 t2_commit_ts 时读应看到 v2
        {
            let client = PercolatorClient::new(&mut cluster, &mut tso);
            assert_eq!(
                client.get(b"a", t2_commit_ts).unwrap(),
                Some(b"v2".to_vec())
            );
        }
    }

    // -----------------------------------------------------------------
    //  18. 并发压力测试（Phase 8.8：Percolator 锁 + 冲突检测）
    // -----------------------------------------------------------------

    /// 确定性 LCG 随机数生成器（可复现测试）
    struct StressRng {
        state: u64,
    }

    impl StressRng {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        /// LCG（Numerical Recipes 参数），返回下一个伪随机 u64
        fn next_u64(&mut self) -> u64 {
            self.state = self
                .state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.state
        }

        /// 返回 [0, n) 范围内的伪随机数
        fn range(&mut self, n: usize) -> usize {
            if n == 0 {
                return 0;
            }
            (self.next_u64() % n as u64) as usize
        }
    }

    /// 压力测试统计信息
    #[derive(Default, Debug)]
    struct StressStats {
        /// 成功提交的事务数
        commits: usize,
        /// 因锁冲突中止的事务数
        aborts_lock: usize,
        /// 因写冲突中止的事务数
        aborts_write: usize,
        /// 因余额不足跳过的事务数
        skipped: usize,
        /// 最大重试次数（用于死锁检测）
        max_retries_seen: usize,
    }

    /// 单笔转账的结果
    enum TransferResult {
        /// 成功提交
        Committed,
        /// 余额不足，跳过
        Skipped,
        /// 超过最大重试次数（模拟死锁检测）
        MaxRetriesExceeded { retries: usize },
    }

    /// 并发转账压力测试模拟器
    ///
    /// 模拟 N 个线程并发执行跨分片转账，验证：
    /// - **无死锁**：所有事务在有限次重试内完成
    /// - **总额守恒**：所有账户余额之和始终不变
    /// - **冲突检测**：锁冲突和写冲突被正确检测
    struct StressSim {
        cluster: ShardCluster,
        tso: TimestampOracle,
        accounts: Vec<Vec<u8>>,
        initial_total: u64,
    }

    impl StressSim {
        /// 创建模拟器并初始化所有账户
        ///
        /// 账户名 `acc0`~`accN`，分布在两个分片（< "m" 和 >= "m"）
        fn new(num_accounts: usize, balance: u64) -> Self {
            let mut cluster = make_cluster();
            let mut tso = TimestampOracle::new();

            let accounts: Vec<Vec<u8>> = (0..num_accounts)
                .map(|i| format!("acc{}", i).into_bytes())
                .collect();

            let initial_total = balance * num_accounts as u64;
            for acc in &accounts {
                let mut client = PercolatorClient::new(&mut cluster, &mut tso);
                let ts = client.begin();
                let val = balance.to_string().into_bytes();
                client
                    .prewrite(&Mutation::put(acc.clone(), val.clone()), acc, ts)
                    .unwrap();
                client
                    .commit(&[Mutation::put(acc.clone(), val)], ts)
                    .unwrap();
            }

            Self {
                cluster,
                tso,
                accounts,
                initial_total,
            }
        }

        /// 执行单笔跨分片转账（带冲突重试 + resolve_lock）
        ///
        /// # 流程
        /// 1. Begin → 获取 start_ts
        /// 2. 读 from/to 余额（遇到锁 → resolve_lock → 重试）
        /// 3. Prewrite from + to（遇到锁 → resolve_lock → 重试；写冲突 → 重试）
        /// 4. Commit → 释放所有锁
        fn transfer(
            &mut self,
            from: usize,
            to: usize,
            amount: u64,
            max_retries: usize,
        ) -> TransferResult {
            let mut retries = 0;
            loop {
                let mut client = PercolatorClient::new(&mut self.cluster, &mut self.tso);
                let ts = client.begin();

                // 读 from 余额
                let from_val = match client.get(&self.accounts[from], ts) {
                    Ok(Some(v)) => v,
                    Ok(None) => return TransferResult::Skipped,
                    Err(TxnError::LockedOnRead { key, .. }) => {
                        let _ = client.resolve_lock(&key);
                        retries += 1;
                        if retries > max_retries {
                            return TransferResult::MaxRetriesExceeded { retries };
                        }
                        continue;
                    }
                    Err(_) => return TransferResult::Skipped,
                };
                let from_bal: u64 = match std::str::from_utf8(&from_val)
                    .ok()
                    .and_then(|s| s.parse().ok())
                {
                    Some(v) => v,
                    None => return TransferResult::Skipped,
                };

                if from_bal < amount {
                    return TransferResult::Skipped;
                }

                // 读 to 余额
                let to_val = match client.get(&self.accounts[to], ts) {
                    Ok(Some(v)) => v,
                    Ok(None) => return TransferResult::Skipped,
                    Err(TxnError::LockedOnRead { key, .. }) => {
                        let _ = client.resolve_lock(&key);
                        retries += 1;
                        if retries > max_retries {
                            return TransferResult::MaxRetriesExceeded { retries };
                        }
                        continue;
                    }
                    Err(_) => return TransferResult::Skipped,
                };
                let to_bal: u64 = match std::str::from_utf8(&to_val)
                    .ok()
                    .and_then(|s| s.parse().ok())
                {
                    Some(v) => v,
                    None => return TransferResult::Skipped,
                };

                // 构造 mutations
                let mutations = vec![
                    Mutation::put(
                        self.accounts[from].clone(),
                        (from_bal - amount).to_string().into_bytes(),
                    ),
                    Mutation::put(
                        self.accounts[to].clone(),
                        (to_bal + amount).to_string().into_bytes(),
                    ),
                ];

                // Prewrite
                match client.prewrite_all(&mutations, ts) {
                    Ok(()) => match client.commit(&mutations, ts) {
                        Ok(_) => return TransferResult::Committed,
                        Err(_) => {
                            let _ = client.rollback(&mutations, ts);
                            retries += 1;
                            if retries > max_retries {
                                return TransferResult::MaxRetriesExceeded { retries };
                            }
                        }
                    },
                    Err(TxnError::KeyAlreadyLocked { key, .. }) => {
                        let _ = client.resolve_lock(&key);
                        let _ = client.rollback(&mutations, ts);
                        retries += 1;
                        if retries > max_retries {
                            return TransferResult::MaxRetriesExceeded { retries };
                        }
                    }
                    Err(TxnError::WriteConflict { .. }) => {
                        let _ = client.rollback(&mutations, ts);
                        retries += 1;
                        if retries > max_retries {
                            return TransferResult::MaxRetriesExceeded { retries };
                        }
                    }
                    Err(_) => return TransferResult::Skipped,
                }
            }
        }

        /// 验证总额守恒：所有账户余额之和 == initial_total
        ///
        /// 读取时若遇到残留锁（来自未完成/已中止事务），先调用 `resolve_lock`
        /// 清理后再重读，确保读到最新已提交值。
        fn verify_total(&mut self) -> u64 {
            let mut total = 0u64;
            for acc in &self.accounts {
                // 最多重试 10 次：遇到锁时先 resolve 再重读
                for _ in 0..10 {
                    let read_ts = self.tso.current();
                    // 读阶段：client 借用 cluster/tso，作用域结束后才能创建 resolver
                    let blocked_key = {
                        let client = PercolatorClient::new(&mut self.cluster, &mut self.tso);
                        match client.get(acc, read_ts) {
                            Ok(Some(v)) => {
                                if let Ok(s) = std::str::from_utf8(&v) {
                                    if let Ok(n) = s.parse::<u64>() {
                                        total += n;
                                    }
                                }
                                None
                            }
                            Ok(None) => None,
                            Err(TxnError::LockedOnRead { key, .. }) => Some(key),
                            Err(_) => None,
                        }
                    };
                    match blocked_key {
                        // 成功读取或不需要重试
                        None => break,
                        // 遇到锁：resolve 后重试
                        Some(key) => {
                            let mut resolver =
                                PercolatorClient::new(&mut self.cluster, &mut self.tso);
                            let _ = resolver.resolve_lock(&key);
                        }
                    }
                }
            }
            total
        }
    }

    /// 运行并发转账压力测试
    ///
    /// 模拟 `num_threads` 个线程，每个执行 `transfers_per_thread` 笔转账。
    /// 使用交错调度（round-robin）模拟并发，每轮从每个线程队列取一笔执行。
    fn run_concurrent_transfers(
        sim: &mut StressSim,
        num_threads: usize,
        transfers_per_thread: usize,
        rng_seed: u64,
        max_retries: usize,
    ) -> StressStats {
        let mut rng = StressRng::new(rng_seed);
        let num_accounts = sim.accounts.len();
        let mut stats = StressStats::default();

        // 为每个线程生成转账队列
        let mut queues: Vec<Vec<(usize, usize, u64)>> = (0..num_threads)
            .map(|_| {
                (0..transfers_per_thread)
                    .map(|_| {
                        let from = rng.range(num_accounts);
                        let to = rng.range(num_accounts);
                        let amount = rng.next_u64() % 100 + 1;
                        (from, to, amount)
                    })
                    .collect()
            })
            .collect();

        // 交错执行：每轮从每个线程取一笔转账
        loop {
            let mut all_empty = true;
            for queue in &mut queues {
                if let Some((from, to, amount)) = queue.pop() {
                    all_empty = false;
                    if from == to {
                        stats.skipped += 1;
                        continue;
                    }
                    match sim.transfer(from, to, amount, max_retries) {
                        TransferResult::Committed => stats.commits += 1,
                        TransferResult::Skipped => stats.skipped += 1,
                        TransferResult::MaxRetriesExceeded { retries } => {
                            stats.aborts_lock += 1;
                            if retries > stats.max_retries_seen {
                                stats.max_retries_seen = retries;
                            }
                        }
                    }
                }
            }
            if all_empty {
                break;
            }
        }

        stats
    }

    /// 运行交错操作级并发测试
    ///
    /// 创建 N 个事务，交错执行它们的 begin/read/prewrite/commit 操作。
    /// 这比事务级交错更能暴露锁竞争和冲突问题。
    fn run_interleaved_transfers(
        sim: &mut StressSim,
        num_txns: usize,
        rng_seed: u64,
        max_retries: usize,
    ) -> StressStats {
        let mut rng = StressRng::new(rng_seed);
        let num_accounts = sim.accounts.len();
        let mut stats = StressStats::default();

        // 生成所有事务的参数
        let txn_params: Vec<(usize, usize, u64)> = (0..num_txns)
            .map(|_| {
                let from = rng.range(num_accounts);
                let to = rng.range(num_accounts);
                let amount = rng.next_u64() % 50 + 1;
                (from, to, amount)
            })
            .collect();

        // 交错执行：每轮推进所有未完成事务各一步
        // 事务状态：(start_ts, from_bal, to_bal, mutations, step, retries)
        #[derive(Clone)]
        enum Step {
            Begin,
            ReadFrom,
            ReadTo,
            Prewrite,
            Commit,
            Done,
        }

        let mut txns: Vec<(u64, u64, u64, Vec<Mutation>, Step, usize)> = txn_params
            .iter()
            .map(|_| (0, 0, 0, Vec::new(), Step::Begin, 0))
            .collect();

        loop {
            let mut all_done = true;
            for (idx, (from, to, amount)) in txn_params.iter().enumerate() {
                let (_, _, _, _, step, _) = &txns[idx];
                if matches!(step, Step::Done) {
                    continue;
                }
                all_done = false;

                let (ts, from_bal, to_bal, mutations, step, retries) = &mut txns[idx];
                match step {
                    Step::Begin => {
                        let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                        *ts = client.begin();
                        *step = Step::ReadFrom;
                    }
                    Step::ReadFrom => {
                        let client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                        match client.get(&sim.accounts[*from], *ts) {
                            Ok(Some(v)) => {
                                *from_bal = std::str::from_utf8(&v)
                                    .ok()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0);
                                *step = Step::ReadTo;
                            }
                            Ok(None) => {
                                stats.skipped += 1;
                                *step = Step::Done;
                            }
                            Err(TxnError::LockedOnRead { key, .. }) => {
                                let mut client =
                                    PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                                let _ = client.resolve_lock(&key);
                                *retries += 1;
                                if *retries > max_retries {
                                    stats.aborts_lock += 1;
                                    *step = Step::Done;
                                } else {
                                    *step = Step::Begin; // 重试
                                }
                            }
                            Err(_) => {
                                stats.skipped += 1;
                                *step = Step::Done;
                            }
                        }
                    }
                    Step::ReadTo => {
                        let client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                        match client.get(&sim.accounts[*to], *ts) {
                            Ok(Some(v)) => {
                                *to_bal = std::str::from_utf8(&v)
                                    .ok()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0);
                                if *from_bal < *amount {
                                    stats.skipped += 1;
                                    *step = Step::Done;
                                } else {
                                    *mutations = vec![
                                        Mutation::put(
                                            sim.accounts[*from].clone(),
                                            (*from_bal - *amount).to_string().into_bytes(),
                                        ),
                                        Mutation::put(
                                            sim.accounts[*to].clone(),
                                            (*to_bal + *amount).to_string().into_bytes(),
                                        ),
                                    ];
                                    *step = Step::Prewrite;
                                }
                            }
                            Ok(None) => {
                                stats.skipped += 1;
                                *step = Step::Done;
                            }
                            Err(TxnError::LockedOnRead { key, .. }) => {
                                let mut client =
                                    PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                                let _ = client.resolve_lock(&key);
                                *retries += 1;
                                if *retries > max_retries {
                                    stats.aborts_lock += 1;
                                    *step = Step::Done;
                                } else {
                                    *step = Step::Begin;
                                }
                            }
                            Err(_) => {
                                stats.skipped += 1;
                                *step = Step::Done;
                            }
                        }
                    }
                    Step::Prewrite => {
                        let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                        match client.prewrite_all(mutations, *ts) {
                            Ok(()) => *step = Step::Commit,
                            Err(TxnError::KeyAlreadyLocked { key, .. }) => {
                                // 解决阻塞锁，并清理本事务已 prewrite 的部分键
                                let _ = client.resolve_lock(&key);
                                let _ = client.rollback(mutations, *ts);
                                *retries += 1;
                                if *retries > max_retries {
                                    stats.aborts_lock += 1;
                                    *step = Step::Done;
                                } else {
                                    *step = Step::Begin;
                                }
                            }
                            Err(TxnError::WriteConflict { .. }) => {
                                // 清理本事务已 prewrite 的部分键
                                let _ = client.rollback(mutations, *ts);
                                *retries += 1;
                                if *retries > max_retries {
                                    stats.aborts_write += 1;
                                    *step = Step::Done;
                                } else {
                                    *step = Step::Begin;
                                }
                            }
                            Err(_) => {
                                let _ = client.rollback(mutations, *ts);
                                stats.skipped += 1;
                                *step = Step::Done;
                            }
                        }
                    }
                    Step::Commit => {
                        let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
                        match client.commit(mutations, *ts) {
                            Ok(_) => {
                                stats.commits += 1;
                                *step = Step::Done;
                            }
                            Err(_) => {
                                *retries += 1;
                                if *retries > max_retries {
                                    stats.aborts_lock += 1;
                                    *step = Step::Done;
                                } else {
                                    *step = Step::Begin;
                                }
                            }
                        }
                    }
                    Step::Done => {}
                }
                if *retries > stats.max_retries_seen {
                    stats.max_retries_seen = *retries;
                }
            }
            if all_done {
                break;
            }
        }

        stats
    }

    #[test]
    fn test_stress_20_threads_total_conservation() {
        // 20 线程 × 10 笔转账 → 总额守恒
        let mut sim = StressSim::new(10, 1000);
        let stats = run_concurrent_transfers(&mut sim, 20, 10, 42, 10);

        // 验证总额守恒
        let total = sim.verify_total();
        assert_eq!(
            total,
            sim.initial_total,
            "总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            sim.initial_total,
            total,
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );
    }

    #[test]
    fn test_stress_no_deadlock() {
        // 20 线程并发 → 无死锁（所有事务在 max_retries 内完成）
        let mut sim = StressSim::new(10, 1000);
        let max_retries = 20;
        let stats = run_concurrent_transfers(&mut sim, 20, 10, 12345, max_retries);

        // 无死锁：aborts 应为 0（所有事务都成功提交或因余额不足跳过）
        assert_eq!(
            stats.aborts_lock,
            0,
            "存在死锁（锁冲突中止 > 0）：commits={}, skips={}, aborts={}/{}, max_retries={}",
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );
        assert_eq!(
            stats.aborts_write,
            0,
            "存在写冲突中止：commits={}, skips={}, aborts={}/{}, max_retries={}",
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );
        // 至少有一些成功提交
        assert!(stats.commits > 0, "无成功提交的事务");
    }

    #[test]
    fn test_stress_interleaved_operations() {
        // 操作级交错：10 个事务交错执行 begin/read/prewrite/commit
        let mut sim = StressSim::new(5, 500);
        let stats = run_interleaved_transfers(&mut sim, 10, 999, 15);

        // 验证总额守恒
        let total = sim.verify_total();
        assert_eq!(
            total, sim.initial_total,
            "交错操作后总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            sim.initial_total, total, stats.commits, stats.skipped,
            stats.aborts_lock, stats.aborts_write, stats.max_retries_seen
        );
    }

    #[test]
    fn test_stress_lock_conflict_detection() {
        // 验证锁冲突被正确检测：两个事务同时 prewrite 同一 key
        let mut sim = StressSim::new(3, 1000);

        // T1: prewrite acc0 → acc1（加锁 acc0 + acc1）
        let t1_ts = {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            let ts = client.begin();
            let from_val = client.get(b"acc0", ts).unwrap().unwrap();
            let from_bal: u64 = std::str::from_utf8(&from_val).unwrap().parse().unwrap();
            let to_val = client.get(b"acc1", ts).unwrap().unwrap();
            let to_bal: u64 = std::str::from_utf8(&to_val).unwrap().parse().unwrap();
            let mutations = vec![
                Mutation::put(b"acc0".to_vec(), (from_bal - 100).to_string().into_bytes()),
                Mutation::put(b"acc1".to_vec(), (to_bal + 100).to_string().into_bytes()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            ts
        };

        // T2: 尝试 prewrite acc0 → acc2（应被 acc0 的锁阻塞）
        {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            let ts = client.begin();
            let result = client.prewrite(
                &Mutation::put(b"acc0".to_vec(), b"800".to_vec()),
                b"acc0",
                ts,
            );
            assert!(
                matches!(result, Err(TxnError::KeyAlreadyLocked { .. })),
                "T2 prewrite 应被 T1 的锁阻塞，实际：{:?}",
                result
            );
        }

        // 回滚 T1
        {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            client
                .rollback(
                    &[
                        Mutation::put(b"acc0".to_vec(), b"900".to_vec()),
                        Mutation::put(b"acc1".to_vec(), b"1100".to_vec()),
                    ],
                    t1_ts,
                )
                .unwrap();
        }

        // T3: 现在可以 prewrite acc0（锁已释放）
        {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            let ts = client.begin();
            let result = client.prewrite(
                &Mutation::put(b"acc0".to_vec(), b"500".to_vec()),
                b"acc0",
                ts,
            );
            assert!(
                matches!(result, Ok(())),
                "T3 prewrite 应成功（锁已释放），实际：{:?}",
                result
            );
        }

        // 验证总额守恒
        let total = sim.verify_total();
        assert_eq!(total, sim.initial_total);
    }

    #[test]
    fn test_stress_large_scale_conservation() {
        // 大规模压力测试：50 线程 × 20 笔 = 1000 笔转账
        let mut sim = StressSim::new(20, 5000);
        let stats = run_concurrent_transfers(&mut sim, 50, 20, 7777, 30);

        let total = sim.verify_total();
        assert_eq!(
            total, sim.initial_total,
            "大规模压力测试总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            sim.initial_total, total, stats.commits, stats.skipped,
            stats.aborts_lock, stats.aborts_write, stats.max_retries_seen
        );
        // 至少 50% 的非跳过事务成功提交
        let attempted = stats.commits + stats.aborts_lock + stats.aborts_write;
        if attempted > 0 {
            let commit_rate = stats.commits as f64 / attempted as f64 * 100.0;
            assert!(
                commit_rate >= 50.0,
                "提交率过低：{:.1}%（commits={}, aborts={}/{}）",
                commit_rate,
                stats.commits,
                stats.aborts_lock,
                stats.aborts_write
            );
        }
    }

    #[test]
    fn test_stress_write_conflict_after_commit() {
        // 验证写冲突检测：T1 commit 后，T2 用旧 start_ts prewrite → WriteConflict
        let mut sim = StressSim::new(2, 1000);

        // T2 先 begin（获取较小的 start_ts）
        let t2_ts = {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            client.begin()
        };

        // T1 begin + prewrite + commit（commit_ts > t2_ts）
        // T1 做一笔 acc0→acc1 转账：acc0=900, acc1=1100（总额守恒）
        {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            let ts = client.begin();
            let mutations = vec![
                Mutation::put(b"acc0".to_vec(), b"900".to_vec()),
                Mutation::put(b"acc1".to_vec(), b"1100".to_vec()),
            ];
            client.prewrite_all(&mutations, ts).unwrap();
            client.commit(&mutations, ts).unwrap();
        }

        // T2 用旧 ts prewrite → WriteConflict（因为 T1 的 commit_ts > t2_ts）
        {
            let mut client = PercolatorClient::new(&mut sim.cluster, &mut sim.tso);
            let result = client.prewrite(
                &Mutation::put(b"acc0".to_vec(), b"500".to_vec()),
                b"acc0",
                t2_ts,
            );
            assert!(
                matches!(result, Err(TxnError::WriteConflict { .. })),
                "T2 应检测到写冲突（T1 已 commit），实际：{:?}",
                result
            );
        }

        // 验证总额守恒
        let total = sim.verify_total();
        assert_eq!(total, sim.initial_total);
    }

    // -----------------------------------------------------------------
    //  19. Jepsen Bank 测试（Phase 8.9：Percolator + Raft 集成）
    // -----------------------------------------------------------------

    /// 已提交事务的记录（用于可串行化验证）
    ///
    /// 记录事务的 start_ts / commit_ts / mutations，
    /// 按 commit_ts 排序后重放即可验证可串行化隔离级别。
    #[derive(Clone, Debug)]
    struct TxnHistoryEntry {
        /// 事务开始时间戳
        #[allow(dead_code)]
        start_ts: u64,
        /// 事务提交时间戳
        commit_ts: u64,
        /// 事务写入的变更
        mutations: Vec<Mutation>,
    }

    /// 单次转账尝试的结果（不包含重试逻辑）
    enum TransferAttempt {
        /// 成功提交（携带时间戳用于历史记录）
        Committed {
            start_ts: u64,
            commit_ts: u64,
            mutations: Vec<Mutation>,
        },
        /// 跳过（余额不足或键不存在）
        Skipped,
        /// 需要重试（锁冲突 / 写冲突 / Raft 不可用）
        Retry,
    }

    /// Jepsen 风格故障注入器
    ///
    /// 在每步操作之间按概率注入网络分区 / 节点崩溃 / 节点恢复，
    /// 同时保证集群始终可用（至少 2 节点可互通以维持 Raft 多数派）。
    struct FaultInjector {
        /// 随机数生成器
        rng: StressRng,
        /// 网络分区概率（0~100，整数百分比）
        partition_prob: u32,
        /// 节点崩溃概率（0~100）
        crash_prob: u32,
        /// 节点恢复概率（0~100）
        restart_prob: u32,
        /// 当前离线的节点集合
        offline_nodes: Vec<u64>,
        /// 当前分区的节点对
        partitions: Vec<(u64, u64)>,
    }

    impl FaultInjector {
        /// 创建故障注入器
        fn new(seed: u64, partition_prob: u32, crash_prob: u32, restart_prob: u32) -> Self {
            Self {
                rng: StressRng::new(seed),
                partition_prob,
                crash_prob,
                restart_prob,
                offline_nodes: Vec::new(),
                partitions: Vec::new(),
            }
        }

        /// 按概率注入故障（网络分区 / 节点崩溃 / 节点恢复）
        ///
        /// 保证同一时刻最多 1 个活跃故障，确保集群始终有 2 节点可互通。
        fn maybe_inject(&mut self, cluster: &ShardCluster) {
            let all_nodes = [1u64, 2, 3];

            // 1. 尝试恢复离线节点（按 restart_prob 概率）
            if !self.offline_nodes.is_empty()
                && self.restart_prob > 0
                && (self.rng.next_u64() % 100) < self.restart_prob as u64
            {
                let idx = self.rng.range(self.offline_nodes.len());
                let node = self.offline_nodes.swap_remove(idx);
                cluster.set_online(node);
            }

            // 2. 尝试恢复网络分区（按 restart_prob 概率）
            //    heal_all 会同时清除离线状态，需重新设置
            if !self.partitions.is_empty()
                && self.restart_prob > 0
                && (self.rng.next_u64() % 100) < self.restart_prob as u64
            {
                let offline_backup = self.offline_nodes.clone();
                cluster.network.heal_all();
                for &node in &offline_backup {
                    cluster.set_offline(node);
                }
                self.partitions.clear();
            }

            // 3. 尝试注入新故障（仅当无活跃故障时，保证集群可用）
            if !self.has_faults() {
                let roll = self.rng.next_u64() % 100;
                if self.crash_prob > 0 && roll < self.crash_prob as u64 {
                    // 崩溃 1 个随机节点
                    let victim = all_nodes[self.rng.range(3)];
                    cluster.set_offline(victim);
                    self.offline_nodes.push(victim);
                } else if self.partition_prob > 0
                    && roll < (self.crash_prob as u64 + self.partition_prob as u64)
                {
                    // 分区 1 对随机节点
                    let a = all_nodes[self.rng.range(3)];
                    let b = loop {
                        let candidate = all_nodes[self.rng.range(3)];
                        if candidate != a {
                            break candidate;
                        }
                    };
                    cluster.network.partition(a, b);
                    self.partitions.push((a, b));
                }
            }
        }

        /// 恢复所有故障
        fn heal_all(&mut self, cluster: &ShardCluster) {
            cluster.network.heal_all();
            self.offline_nodes.clear();
            self.partitions.clear();
        }

        /// 是否有活跃故障
        fn has_faults(&self) -> bool {
            !self.offline_nodes.is_empty() || !self.partitions.is_empty()
        }
    }

    /// Jepsen Bank 测试夹具
    ///
    /// 模拟 3 节点集群上的银行转账工作负载，支持：
    /// - 并发转账（交错调度）
    /// - 故障注入（网络分区 + 节点崩溃）
    /// - 可串行化验证（按 commit_ts 重放事务历史）
    struct JepsenBank {
        /// 3 节点 2 分片集群
        cluster: ShardCluster,
        /// 全局时间戳服务
        tso: TimestampOracle,
        /// 账户键列表（acc0~accN）
        accounts: Vec<Vec<u8>>,
        /// 初始总额（用于守恒验证）
        initial_total: u64,
        /// 已提交事务历史（按 commit_ts 递增）
        history: Vec<TxnHistoryEntry>,
        /// 故障注入器
        injector: FaultInjector,
        /// 工作负载随机数生成器
        workload_rng: StressRng,
    }

    impl JepsenBank {
        /// 创建 Bank 测试夹具并初始化所有账户
        ///
        /// 账户名 `acc0`~`accN`，每账户初始余额 `balance`，分布在两个分片。
        /// 初始化事务也记录到 history，确保可串行化验证从空状态开始重放。
        fn new(
            num_accounts: usize,
            balance: u64,
            seed: u64,
            partition_prob: u32,
            crash_prob: u32,
            restart_prob: u32,
        ) -> Self {
            let mut cluster = make_cluster();
            let mut tso = TimestampOracle::new();

            let accounts: Vec<Vec<u8>> = (0..num_accounts)
                .map(|i| format!("acc{}", i).into_bytes())
                .collect();

            let initial_total = balance * num_accounts as u64;
            let mut history = Vec::new();

            // 初始化所有账户余额（每笔作为独立事务提交并记录到历史）
            for acc in &accounts {
                let val = balance.to_string().into_bytes();
                let (start_ts, commit_ts) = {
                    let mut client = PercolatorClient::new(&mut cluster, &mut tso);
                    let ts = client.begin();
                    client
                        .prewrite(&Mutation::put(acc.clone(), val.clone()), acc, ts)
                        .unwrap();
                    let cts = client
                        .commit(&[Mutation::put(acc.clone(), val.clone())], ts)
                        .unwrap();
                    (ts, cts)
                };
                history.push(TxnHistoryEntry {
                    start_ts,
                    commit_ts,
                    mutations: vec![Mutation::put(acc.clone(), val)],
                });
            }

            Self {
                cluster,
                tso,
                accounts,
                initial_total,
                history,
                injector: FaultInjector::new(seed, partition_prob, crash_prob, restart_prob),
                workload_rng: StressRng::new(seed.wrapping_add(7777)),
            }
        }

        /// 读取单个账户余额（遇到锁时 resolve 后重试）
        ///
        /// 返回原始字节值，账户不存在时返回 None。
        fn read_account(&mut self, acc: &[u8]) -> Option<Vec<u8>> {
            for _ in 0..10 {
                let read_ts = self.tso.current();
                // 读阶段：client 借用 cluster/tso，作用域结束后才能创建 resolver
                let blocked_key = {
                    let client = PercolatorClient::new(&mut self.cluster, &mut self.tso);
                    match client.get(acc, read_ts) {
                        Ok(Some(v)) => return Some(v),
                        Ok(None) => return None,
                        Err(TxnError::LockedOnRead { key, .. }) => Some(key),
                        Err(_) => return None,
                    }
                };
                // 遇到锁：resolve 后推进 Raft 时钟让提议被应用，再重试
                if let Some(key) = blocked_key {
                    let mut resolver = PercolatorClient::new(&mut self.cluster, &mut self.tso);
                    let _ = resolver.resolve_lock(&key);
                    // 让 resolve_lock 的 put/delete 提议被 Raft 复制并应用到状态机
                    self.cluster.run_for(300);
                }
            }
            None
        }

        /// 单次转账尝试（不包含重试逻辑）
        ///
        /// 执行完整的 Begin → Read → Prewrite → Commit 流程，
        /// 遇到锁冲突 / 写冲突 / Raft 不可用时返回 Retry。
        fn try_transfer_once(&mut self, from: usize, to: usize, amount: u64) -> TransferAttempt {
            let mut client = PercolatorClient::new(&mut self.cluster, &mut self.tso);
            let start_ts = client.begin();

            // 读 from 余额
            let from_val = match client.get(&self.accounts[from], start_ts) {
                Ok(Some(v)) => v,
                Ok(None) => return TransferAttempt::Skipped,
                Err(TxnError::LockedOnRead { key, .. }) => {
                    let _ = client.resolve_lock(&key);
                    return TransferAttempt::Retry;
                }
                Err(TxnError::Raft(_)) => return TransferAttempt::Retry,
                Err(_) => return TransferAttempt::Skipped,
            };
            let from_bal: u64 = std::str::from_utf8(&from_val)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            if from_bal < amount {
                return TransferAttempt::Skipped;
            }

            // 读 to 余额
            let to_val = match client.get(&self.accounts[to], start_ts) {
                Ok(Some(v)) => v,
                Ok(None) => return TransferAttempt::Skipped,
                Err(TxnError::LockedOnRead { key, .. }) => {
                    let _ = client.resolve_lock(&key);
                    return TransferAttempt::Retry;
                }
                Err(TxnError::Raft(_)) => return TransferAttempt::Retry,
                Err(_) => return TransferAttempt::Skipped,
            };
            let to_bal: u64 = std::str::from_utf8(&to_val)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            // 构造 mutations
            let mutations = vec![
                Mutation::put(
                    self.accounts[from].clone(),
                    (from_bal - amount).to_string().into_bytes(),
                ),
                Mutation::put(
                    self.accounts[to].clone(),
                    (to_bal + amount).to_string().into_bytes(),
                ),
            ];

            // Prewrite + Commit
            match client.prewrite_all(&mutations, start_ts) {
                Ok(()) => match client.commit(&mutations, start_ts) {
                    Ok(commit_ts) => {
                        // 修复阶段：确保所有 DATA 记录、COMMIT 写记录已应用且所有锁已删除。
                        // 原因 1：prewrite() 内部 put(DATA) 和 put(LOCK) 是独立的 Raft
                        //   提议，若在两者之间 Leader 变更，DATA 提议可能丢失而 LOCK 被新
                        //   Leader 应用。此时 COMMIT 写记录存在但 DATA 缺失，读取时 get()
                        //   返回 Ok(None)（被当作余额 0），导致金额凭空消失。
                        // 原因 2：commit() 内部 put(COMMIT) 和 delete(lock) 是独立的 Raft
                        //   提议，若在两者之间 Leader 变更，COMMIT 提议可能丢失而 lock
                        //   被删除，导致数据不一致。
                        // 关键：commit() 返回 Ok 意味着事务已提交，必须确保状态机最终
                        // 反映这一点，否则重试会导致 primary 被重复扣款而 secondary 丢失。
                        let mut all_applied = true;
                        for m in &mutations {
                            let shard = match client.route(m.key()) {
                                Ok(s) => s,
                                Err(_) => {
                                    all_applied = false;
                                    break;
                                }
                            };
                            // 1. 验证 DATA 记录已应用（仅 Put 操作）
                            if let Mutation::Put { value, .. } = m {
                                let dkey = data_key(m.key(), start_ts);
                                let mut data_applied = false;
                                for _ in 0..20 {
                                    if client.cluster.get_from_leader(shard, &dkey).is_some() {
                                        data_applied = true;
                                        break;
                                    }
                                    // DATA 记录丢失，从 mutation 的 value 重新写入
                                    let _ = client.cluster.put(shard, dkey.clone(), value.clone());
                                    client.cluster.run_for(500);
                                }
                                if !data_applied {
                                    all_applied = false;
                                    break;
                                }
                            }
                            // 2. 验证 COMMIT 写记录已应用
                            let wkey = write_key(m.key(), commit_ts);
                            let mut applied = false;
                            for _ in 0..20 {
                                if client.cluster.get_from_leader(shard, &wkey).is_some() {
                                    applied = true;
                                    break;
                                }
                                // 重新提议 COMMIT 写记录（可能因无 Leader 返回 Err，忽略）
                                let wrecord = WriteRecord {
                                    start_ts,
                                    kind: m.write_kind(),
                                };
                                let _ = client.cluster.put(shard, wkey.clone(), wrecord.encode());
                                client.cluster.run_for(500);
                            }
                            if !applied {
                                all_applied = false;
                                break;
                            }
                            // 3. 清理残留锁（commit 可能未删除 lock）
                            let lkey = lock_key(m.key());
                            for _ in 0..10 {
                                if client.cluster.get_from_leader(shard, &lkey).is_none() {
                                    break;
                                }
                                let _ = client.cluster.delete(shard, lkey.clone());
                                client.cluster.run_for(300);
                            }
                        }
                        if all_applied {
                            TransferAttempt::Committed {
                                start_ts,
                                commit_ts,
                                mutations,
                            }
                        } else {
                            // 极端情况：20 次重试后仍无法应用 COMMIT。
                            // 不记录到 history，返回 Retry。
                            // 残留锁由后续 read_account/resolve_lock 正确前推或回滚。
                            TransferAttempt::Retry
                        }
                    }
                    Err(_) => {
                        // 注意：不能调用 rollback！commit 失败时 primary 可能已提交，
                        // 此时 rollback 会写入 ROLLBACK 记录覆盖 COMMIT 状态，导致数据损坏。
                        // 残留的 secondary 锁会由后续 read_account/resolve_lock 正确前推或回滚。
                        TransferAttempt::Retry
                    }
                },
                Err(TxnError::KeyAlreadyLocked { key, .. }) => {
                    let _ = client.resolve_lock(&key);
                    let _ = client.rollback(&mutations, start_ts);
                    TransferAttempt::Retry
                }
                Err(TxnError::WriteConflict { .. }) => {
                    let _ = client.rollback(&mutations, start_ts);
                    TransferAttempt::Retry
                }
                Err(TxnError::Raft(_)) => {
                    // prewrite 阶段失败：primary 未提交，可安全 rollback 清理残留锁
                    let _ = client.rollback(&mutations, start_ts);
                    TransferAttempt::Retry
                }
                Err(_) => TransferAttempt::Skipped,
            }
        }

        /// 执行单笔带故障注入的转账（成功时记录到 history）
        ///
        /// 内部调用 `try_transfer_once` 并处理重试，超过 max_retries 时返回失败。
        /// 每次重试前调用 `run_for` 推进 Raft 时钟，让选举/复制有机会推进。
        fn transfer(
            &mut self,
            from: usize,
            to: usize,
            amount: u64,
            max_retries: usize,
        ) -> TransferResult {
            let mut retries = 0;
            loop {
                match self.try_transfer_once(from, to, amount) {
                    TransferAttempt::Committed {
                        start_ts,
                        commit_ts,
                        mutations,
                    } => {
                        self.history.push(TxnHistoryEntry {
                            start_ts,
                            commit_ts,
                            mutations,
                        });
                        return TransferResult::Committed;
                    }
                    TransferAttempt::Skipped => return TransferResult::Skipped,
                    TransferAttempt::Retry => {
                        retries += 1;
                        if retries > max_retries {
                            return TransferResult::MaxRetriesExceeded { retries };
                        }
                        // 重试前推进 Raft 时钟，让选举/复制/锁清理有机会推进
                        self.cluster.run_for(300);
                    }
                }
            }
        }

        /// 确保集群处于无故障状态（heal_all + 等待 Raft 恢复）
        fn ensure_clean_state(&mut self) {
            self.injector.heal_all(&self.cluster);
            self.cluster.run_for(2000);
        }

        /// 验证总额守恒：所有账户余额之和
        fn verify_total(&mut self) -> u64 {
            self.ensure_clean_state();
            // 克隆账户列表避免借用冲突（read_account 需 &mut self）
            let accounts = self.accounts.clone();
            let mut total = 0u64;
            for acc in &accounts {
                if let Some(v) = self.read_account(acc) {
                    if let Ok(s) = std::str::from_utf8(&v) {
                        if let Ok(n) = s.parse::<u64>() {
                            total += n;
                        }
                    }
                }
            }
            total
        }

        /// 验证所有账户余额 >= 0
        fn verify_non_negative(&mut self) -> bool {
            self.ensure_clean_state();
            let accounts = self.accounts.clone();
            for acc in &accounts {
                if let Some(v) = self.read_account(acc) {
                    if let Ok(s) = std::str::from_utf8(&v) {
                        if let Ok(n) = s.parse::<i64>() {
                            if n < 0 {
                                return false;
                            }
                        }
                    }
                }
            }
            true
        }

        /// 验证所有账户都能读到
        fn verify_account_set(&mut self) -> bool {
            self.ensure_clean_state();
            let accounts = self.accounts.clone();
            for acc in &accounts {
                if self.read_account(acc).is_none() {
                    return false;
                }
            }
            true
        }

        /// 可串行化验证（核心）：按 commit_ts 重放事务历史，比较重放状态与集群状态
        ///
        /// 算法：
        /// 1. 按 commit_ts 升序排序 history
        /// 2. 创建独立 HashMap 重放每个事务的 mutations
        /// 3. 读取集群中每个账户的最新值
        /// 4. 比较重放状态 == 集群状态
        fn verify_serializable(&mut self) -> bool {
            self.ensure_clean_state();

            // 1. 按 commit_ts 排序（Percolator 的 commit_ts 全局单调递增，history 本身有序）
            let mut sorted = self.history.clone();
            sorted.sort_by_key(|e| e.commit_ts);

            // 2. 重放到独立 HashMap
            let mut state: std::collections::HashMap<Vec<u8>, Vec<u8>> =
                std::collections::HashMap::new();
            for entry in &sorted {
                for m in &entry.mutations {
                    match m {
                        Mutation::Put { key, value } => {
                            state.insert(key.clone(), value.clone());
                        }
                        Mutation::Delete { key } => {
                            state.remove(key);
                        }
                    }
                }
            }

            // 3. 比较集群状态与重放状态
            let accounts = self.accounts.clone();
            for acc in &accounts {
                let cluster_val = self.read_account(acc);
                let replay_val = state.get(acc).cloned();
                if cluster_val != replay_val {
                    return false;
                }
            }
            true
        }

        /// 运行交错转账工作负载，每步注入故障
        ///
        /// 生成 `num_txns` 笔随机转账（from/to/amount 由 workload_rng 决定），
        /// 每笔转账前调用 `maybe_inject` 注入故障。
        fn run_workload(&mut self, num_txns: usize, max_retries: usize) -> StressStats {
            let num_accounts = self.accounts.len();
            let mut stats = StressStats::default();

            for _ in 0..num_txns {
                // 每步注入故障 + 等待 Raft 选举/复制推进
                self.injector.maybe_inject(&self.cluster);
                self.cluster.run_for(500);

                // 生成转账参数
                let from = self.workload_rng.range(num_accounts);
                let to = self.workload_rng.range(num_accounts);
                let amount = self.workload_rng.next_u64() % 100 + 1;

                if from == to {
                    stats.skipped += 1;
                    continue;
                }

                match self.transfer(from, to, amount, max_retries) {
                    TransferResult::Committed => stats.commits += 1,
                    TransferResult::Skipped => stats.skipped += 1,
                    TransferResult::MaxRetriesExceeded { retries } => {
                        stats.aborts_lock += 1;
                        if retries > stats.max_retries_seen {
                            stats.max_retries_seen = retries;
                        }
                    }
                }
            }

            stats
        }
    }

    /// Jepsen Bank 无故障基线测试
    ///
    /// 3 节点 5 账户，100 笔转账，无故障注入。
    /// 验证：总额守恒 + 余额非负 + 账户集完整 + 可串行化。
    #[test]
    fn test_jepsen_bank_basic() {
        let mut bank = JepsenBank::new(5, 1000, 100, 0, 0, 0);
        let stats = bank.run_workload(100, 10);

        // 验证总额守恒
        let total = bank.verify_total();
        assert_eq!(
            total,
            bank.initial_total,
            "总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            bank.initial_total,
            total,
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );

        // 验证余额非负
        assert!(bank.verify_non_negative(), "存在负余额");

        // 验证账户集完整
        assert!(bank.verify_account_set(), "账户集不完整");

        // 验证可串行化
        assert!(bank.verify_serializable(), "可串行化验证失败");

        // 至少有一些成功提交
        assert!(stats.commits > 0, "无成功提交的事务");
    }

    /// Jepsen Bank 网络分区测试
    ///
    /// 3 节点 5 账户，100 笔转账，partition_prob=15%。
    /// 工作负载结束后恢复所有分区，验证总额守恒 + 可串行化。
    #[test]
    fn test_jepsen_bank_with_partition() {
        let mut bank = JepsenBank::new(5, 1000, 200, 15, 0, 0);
        let stats = bank.run_workload(100, 20);

        // 恢复后验证总额守恒
        let total = bank.verify_total();
        assert_eq!(
            total, bank.initial_total,
            "分区后总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            bank.initial_total,
            total,
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );

        // 验证余额非负
        assert!(bank.verify_non_negative(), "分区后存在负余额");

        // 验证可串行化
        assert!(bank.verify_serializable(), "分区后可串行化验证失败");
    }

    /// Jepsen Bank 节点崩溃测试
    ///
    /// 3 节点 5 账户，100 笔转账，crash_prob=10%，restart_prob=30%。
    /// 工作负载结束后恢复所有节点，验证总额守恒 + 可串行化。
    #[test]
    fn test_jepsen_bank_with_node_crash() {
        let mut bank = JepsenBank::new(5, 1000, 300, 0, 10, 30);
        let stats = bank.run_workload(100, 20);

        // 恢复后验证总额守恒
        let total = bank.verify_total();
        assert_eq!(
            total, bank.initial_total,
            "节点崩溃后总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            bank.initial_total,
            total,
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );

        // 验证余额非负
        assert!(bank.verify_non_negative(), "节点崩溃后存在负余额");

        // 验证可串行化
        assert!(bank.verify_serializable(), "节点崩溃后可串行化验证失败");
    }

    /// Jepsen Bank 组合故障测试
    ///
    /// 3 节点 10 账户，200 笔转账，partition_prob=10%，crash_prob=8%，restart_prob=25%。
    /// 组合网络分区 + 节点崩溃，验证总额守恒 + 可串行化。
    #[test]
    fn test_jepsen_bank_with_combined_faults() {
        let mut bank = JepsenBank::new(10, 1000, 400, 10, 8, 25);
        let stats = bank.run_workload(200, 20);

        // 恢复后验证总额守恒
        let total = bank.verify_total();
        assert_eq!(
            total, bank.initial_total,
            "组合故障后总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            bank.initial_total,
            total,
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );

        // 验证余额非负
        assert!(bank.verify_non_negative(), "组合故障后存在负余额");

        // 验证可串行化
        assert!(bank.verify_serializable(), "组合故障后可串行化验证失败");
    }

    /// Jepsen Bank 可串行化专项测试
    ///
    /// 3 节点 5 账户，50 笔转账，无故障。
    /// 正向：verify_serializable() 返回 true。
    /// 反向：手动构造违反可串行化的 history，verify_serializable() 返回 false。
    #[test]
    fn test_jepsen_bank_serializable_verification() {
        let mut bank = JepsenBank::new(5, 500, 500, 0, 0, 0);
        let stats = bank.run_workload(50, 10);

        // 正向：可串行化验证通过
        assert!(
            bank.verify_serializable(),
            "可串行化验证失败（commits={}, skips={}, aborts={}/{}）",
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write
        );
        assert!(stats.commits > 0, "无成功提交的事务");

        // 反向：构造违反可串行化的 history（插入一个集群中不存在的虚假事务）
        bank.history.push(TxnHistoryEntry {
            start_ts: 999_999,
            commit_ts: 9_999_999,
            mutations: vec![Mutation::put(b"acc0".to_vec(), b"999999".to_vec())],
        });
        assert!(
            !bank.verify_serializable(),
            "违反可串行化的 history 应验证失败"
        );
    }

    /// Jepsen Bank 长时间运行测试
    ///
    /// 3 节点 20 账户，500 笔转账，partition_prob=5%，crash_prob=3%，restart_prob=20%。
    /// 验证大规模下总额守恒 + 可串行化。
    #[test]
    fn test_jepsen_bank_long_run() {
        let mut bank = JepsenBank::new(20, 5000, 600, 5, 3, 20);
        let stats = bank.run_workload(500, 20);

        // 验证总额守恒
        let total = bank.verify_total();
        assert_eq!(
            total, bank.initial_total,
            "长时间运行总额不守恒：初始 {}，最终 {}（commits={}, skips={}, aborts={}/{}, max_retries={}）",
            bank.initial_total,
            total,
            stats.commits,
            stats.skipped,
            stats.aborts_lock,
            stats.aborts_write,
            stats.max_retries_seen
        );

        // 验证余额非负
        assert!(bank.verify_non_negative(), "长时间运行后存在负余额");

        // 验证可串行化
        assert!(bank.verify_serializable(), "长时间运行后可串行化验证失败");
    }
}
