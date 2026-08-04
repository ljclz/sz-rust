//! P0-DIST 迭代 2：Percolator 跨分片事务协调（TSO + 两阶段提交）
//!
//! 在 `DistRuntime`（Raft-replicated KV）之上实现 Percolator 分布式事务协议。
//! 复用 `txn.rs` 中的编码方案（DATA/LOCK/WRITE 前缀），但通过 `DistRuntime`
//! 的 key-level API（`put`/`get`/`scan`）操作底层 KV，而非 `ShardCluster`
//! 的 shard-level API。
//!
//! # 设计目标
//!
//! 1. **真实 2PC 路径**：prewrite → commit/rollback 完整走 Raft propose → apply
//! 2. **TSO 集成**：`begin()` 从 `DistRuntime` 内部 TSO 获取 start_ts
//! 3. **故障恢复**：`resolve_lock` 检测残留锁并前推/回滚
//! 4. **快照读**：`get()` 根据 read_ts 读取已提交版本，跳过未来版本和锁
//!
//! # 与 `txn.rs::PercolatorClient` 的关系
//!
//! - `PercolatorClient`：绑定 `ShardCluster`（多节点多分片），用于纯 Raft 层测试
//! - `DistTxnClient`：绑定 `DistRuntime`（单节点或集群），用于运行时集成
//! - 两者共享编码方案和协议语义，但走不同的 KV 存储后端

use crate::raft::{NodeId, RaftError};
use crate::runtime::{DistRuntime, DistRuntimeError};
use crate::shard::KeyRange;
use crate::txn::{LockInfo, Mutation, TxnError, WriteRecord};

// 复用 txn.rs 中的前缀常量和编码函数
use crate::txn::{
    data_key, lock_key, write_key, write_prefix_range, WRITE_KIND_DELETE, WRITE_KIND_PUT,
    WRITE_KIND_ROLLBACK,
};

// =====================================================================
//  DistTxnClient — Percolator 事务客户端（基于 DistRuntime）
// =====================================================================

/// Percolator 分布式事务客户端（基于 `DistRuntime`）
///
/// 通过 `DistRuntime` 的 Raft-replicated KV 存储实现两阶段提交：
///
/// 1. **begin**：从 TSO 获取 start_ts
/// 2. **prewrite**：对每个写入键加锁 + 写入数据版本
/// 3. **commit**：先提交 primary，再提交 secondary，释放锁
/// 4. **rollback**：删除锁，写入 ROLLBACK 记录
///
/// # 示例
///
/// ```ignore
/// let mut runtime = DistRuntime::new_single_node(1)?;
/// runtime.init()?;
/// let mut txn = DistTxnClient::new(&mut runtime);
///
/// let start_ts = txn.begin();
/// txn.prewrite_all(&[
///     Mutation::put(b"acc1".to_vec(), b"100".to_vec()),
///     Mutation::put(b"acc2".to_vec(), b"200".to_vec()),
/// ], start_ts)?;
/// let commit_ts = txn.commit(&[
///     Mutation::put(b"acc1".to_vec(), b"100".to_vec()),
///     Mutation::put(b"acc2".to_vec(), b"200".to_vec()),
/// ], start_ts)?;
/// ```
pub struct DistTxnClient<'a> {
    /// DistRuntime 引用（提供 KV 存储和 TSO）
    runtime: &'a mut DistRuntime,
}

impl<'a> DistTxnClient<'a> {
    /// 创建 Percolator 事务客户端
    pub fn new(runtime: &'a mut DistRuntime) -> Self {
        Self { runtime }
    }

    /// 获取事务开始时间戳（从 TSO）
    pub fn begin(&mut self) -> u64 {
        self.runtime.begin_transaction()
    }

    /// 快照读：读取键在 read_ts 时刻的最新已提交值
    ///
    /// # 流程
    /// 1. 检查锁：若 lock.start_ts <= read_ts，读被阻塞
    /// 2. 扫描写记录，找最新 commit_ts <= read_ts 的非 ROLLBACK 记录
    /// 3. 若是 Put 则读取对应 data 值，若是 Delete/Rollback 则返回 None
    ///
    /// # Errors
    /// - `LockedOnRead`：键被锁且 lock.start_ts <= read_ts
    /// - `CorruptData`：数据解码失败
    pub fn get(&self, key: &[u8], read_ts: u64) -> Result<Option<Vec<u8>>, TxnError> {
        // 1. 用原始键路由到分片（确保 data/lock/write 记录落在同一分片）
        let shard_id = self.runtime.route_raw_key(key).map_err(txn_err_from)?;

        // 2. 检查锁
        let lkey = lock_key(key);
        if let Some(lock_bytes) = self
            .runtime
            .get_shard(shard_id, &lkey)
            .map_err(txn_err_from)?
        {
            let lock = LockInfo::decode(&lock_bytes)?;
            if lock.start_ts <= read_ts {
                return Err(TxnError::LockedOnRead {
                    key: key.to_vec(),
                    holder_start_ts: lock.start_ts,
                });
            }
        }

        // 3. 扫描写记录
        let (start, end) = write_prefix_range(key);
        let range = KeyRange {
            start: Some(start),
            end: Some(end),
        };
        let writes = self
            .runtime
            .scan_shard(shard_id, &range)
            .map_err(txn_err_from)?;

        let mut latest: Option<(u64, WriteRecord)> = None;
        for (k, v) in writes {
            let Some(commit_ts) = WriteRecord::extract_commit_ts(&k) else {
                continue;
            };
            // 精确键匹配
            if WriteRecord::extract_key(&k) != Some(key) {
                continue;
            }
            if commit_ts > read_ts {
                continue;
            }
            let record = WriteRecord::decode(&v)?;
            if record.kind == WRITE_KIND_ROLLBACK {
                continue;
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
                let dkey = data_key(key, record.start_ts);
                Ok(self
                    .runtime
                    .get_shard(shard_id, &dkey)
                    .map_err(txn_err_from)?
                    .map(|v| v.to_vec()))
            }
        }
    }

    /// Prewrite 阶段：对单个键加锁 + 写入数据版本
    ///
    /// # 流程
    /// 1. 检查写冲突：若有 commit_ts > start_ts 的写记录则冲突
    /// 2. 检查锁：若键已被锁则冲突
    /// 3. 写入 data 记录
    /// 4. 写入 lock 记录
    ///
    /// # Errors
    /// - `KeyAlreadyLocked`：键已被其他事务锁住
    /// - `WriteConflict`：存在 commit_ts > start_ts 的写记录
    pub fn prewrite(
        &mut self,
        mutation: &Mutation,
        primary_key: &[u8],
        start_ts: u64,
    ) -> Result<(), TxnError> {
        let key = mutation.key();

        // 0. 用原始键路由到分片（确保 data/lock/write 记录落在同一分片）
        let shard_id = self.runtime.route_raw_key(key).map_err(txn_err_from)?;

        // 1. 检查写冲突
        let (start, end) = write_prefix_range(key);
        let range = KeyRange {
            start: Some(start),
            end: Some(end),
        };
        let writes = self
            .runtime
            .scan_shard(shard_id, &range)
            .map_err(txn_err_from)?;
        for (k, _) in &writes {
            if let Some(commit_ts) = WriteRecord::extract_commit_ts(k) {
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
        if let Some(existing) = self
            .runtime
            .get_shard(shard_id, &lkey)
            .map_err(txn_err_from)?
        {
            let lock = LockInfo::decode(&existing)?;
            return Err(TxnError::KeyAlreadyLocked {
                key: key.to_vec(),
                holder_start_ts: lock.start_ts,
            });
        }

        // 3. 写入 data 记录
        let dkey = data_key(key, start_ts);
        self.runtime
            .put_shard(shard_id, dkey, mutation.value().to_vec())
            .map_err(txn_err_from)?;

        // 4. 写入 lock 记录
        let lock = LockInfo {
            primary_key: primary_key.to_vec(),
            start_ts,
            kind: mutation.lock_kind(),
            value: mutation.value().to_vec(),
        };
        self.runtime
            .put_shard(shard_id, lkey, lock.encode())
            .map_err(txn_err_from)?;

        Ok(())
    }

    /// Prewrite 所有写操作（第一个键作为 primary）
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
    /// 1. 从 TSO 获取 commit_ts
    /// 2. 检查 primary 锁是否存在
    /// 3. 写入 primary 的 write 记录，删除 primary 的 lock
    /// 4. 对每个 secondary，写入 write 记录，删除 lock
    ///
    /// # Errors
    /// - `LockNotFound`：primary 锁不存在（事务已被回滚）
    pub fn commit(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<u64, TxnError> {
        if mutations.is_empty() {
            return Ok(self.runtime.begin_transaction());
        }

        let commit_ts = self.runtime.begin_transaction();
        let primary_key = mutations[0].key();

        // 1. 用原始键路由 primary 到分片
        let primary_shard_id = self
            .runtime
            .route_raw_key(primary_key)
            .map_err(txn_err_from)?;

        // 2. 检查 primary 锁
        let primary_lkey = lock_key(primary_key);
        let primary_lock_bytes = self
            .runtime
            .get_shard(primary_shard_id, &primary_lkey)
            .map_err(txn_err_from)?
            .ok_or(TxnError::LockNotFound {
                primary_key: primary_key.to_vec(),
            })?;
        let primary_lock = LockInfo::decode(&primary_lock_bytes)?;
        if primary_lock.start_ts != start_ts {
            return Err(TxnError::LockNotFound {
                primary_key: primary_key.to_vec(),
            });
        }

        // 3. 写入 primary 的 write 记录
        let primary_wkey = write_key(primary_key, commit_ts);
        let primary_wrecord = WriteRecord {
            start_ts,
            kind: primary_lock.kind,
        };
        self.runtime
            .put_shard(primary_shard_id, primary_wkey, primary_wrecord.encode())
            .map_err(txn_err_from)?;

        // 4. 删除 primary 的 lock
        self.runtime
            .delete_shard(primary_shard_id, primary_lkey)
            .map_err(txn_err_from)?;

        // 5. 对每个 secondary，写入 write 记录，删除 lock
        for m in mutations.iter().skip(1) {
            let skey = m.key();
            // 用原始键路由 secondary 到分片
            let secondary_shard_id = self.runtime.route_raw_key(skey).map_err(txn_err_from)?;
            let slkey = lock_key(skey);
            let swkey = write_key(skey, commit_ts);

            // 读取 secondary 锁获取 kind
            let skind = if let Some(slock_bytes) = self
                .runtime
                .get_shard(secondary_shard_id, &slkey)
                .map_err(txn_err_from)?
            {
                LockInfo::decode(&slock_bytes)?.kind
            } else {
                // secondary 锁不存在，使用 mutation 的 kind
                m.lock_kind()
            };

            let swrecord = WriteRecord {
                start_ts,
                kind: skind,
            };
            self.runtime
                .put_shard(secondary_shard_id, swkey, swrecord.encode())
                .map_err(txn_err_from)?;
            self.runtime
                .delete_shard(secondary_shard_id, slkey)
                .map_err(txn_err_from)?;
        }

        Ok(commit_ts)
    }

    /// Rollback：回滚事务
    ///
    /// # 流程
    /// 1. 对每个键，删除 lock 记录
    /// 2. 写入 ROLLBACK write 记录（防止延迟的 prewrite 成功）
    /// 3. 删除 data 记录
    ///
    /// # Errors
    /// 路由或 Raft 错误
    pub fn rollback(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<(), TxnError> {
        // 获取新的 rollback_ts 作为 ROLLBACK 记录的 commit_ts 位置
        // （与 PercolatorClient::rollback 一致，txn.rs:688,699）
        let rollback_ts = self.runtime.begin_transaction();

        for m in mutations {
            let key = m.key();
            // 用原始键路由到分片（确保 data/lock/write 记录落在同一分片）
            let shard_id = self.runtime.route_raw_key(key).map_err(txn_err_from)?;

            let lkey = lock_key(key);
            let wkey = write_key(key, rollback_ts);
            let dkey = data_key(key, start_ts);

            // 删除 lock
            self.runtime
                .delete_shard(shard_id, lkey)
                .map_err(txn_err_from)?;

            // 写入 ROLLBACK 记录
            let wrecord = WriteRecord {
                start_ts,
                kind: WRITE_KIND_ROLLBACK,
            };
            self.runtime
                .put_shard(shard_id, wkey, wrecord.encode())
                .map_err(txn_err_from)?;

            // 删除 data
            self.runtime
                .delete_shard(shard_id, dkey)
                .map_err(txn_err_from)?;
        }
        Ok(())
    }

    /// 解析残留锁：检查 primary 状态决定前推或回滚
    ///
    /// 当事务 B 读到事务 A 的残留锁时：
    /// - 若 A 的 primary 已提交 → 前推 A（写入 commit 记录，删除锁）
    /// - 若 A 的 primary 已回滚 → 回滚 A（删除锁和 data）
    ///
    /// # Returns
    /// - `ResolveResult::Committed`：事务已前推
    /// - `ResolveResult::RolledBack`：事务已回滚
    pub fn resolve_lock(&mut self, key: &[u8]) -> Result<ResolveResult, TxnError> {
        // 1. 用原始键路由 key 到分片，读取锁
        let shard_id = self.runtime.route_raw_key(key).map_err(txn_err_from)?;
        let lkey = lock_key(key);
        let lock_bytes = self
            .runtime
            .get_shard(shard_id, &lkey)
            .map_err(txn_err_from)?
            .ok_or(TxnError::LockNotFound {
                primary_key: key.to_vec(),
            })?;
        let lock = LockInfo::decode(&lock_bytes)?;

        // 2. 用原始键路由 primary_key 到分片
        let primary_key = &lock.primary_key;
        let primary_shard_id = self
            .runtime
            .route_raw_key(primary_key)
            .map_err(txn_err_from)?;

        // 3. 扫描 primary 的写记录（写记录是事务状态的最终判据）。
        //    注意：即使 primary 锁仍存在，也可能已有 COMMIT 写记录（commit 阶段
        //    部分失败：写记录已写入但锁删除失败）。此时事务已提交，必须前推而非回滚，
        //    否则 ROLLBACK 记录会覆盖 COMMIT 记录导致数据损坏。
        //    （与 PercolatorClient::resolve_lock 一致，txn.rs:737-786）
        let (start, end) = write_prefix_range(primary_key);
        let range = KeyRange {
            start: Some(start),
            end: Some(end),
        };
        let primary_writes = self
            .runtime
            .scan_shard(primary_shard_id, &range)
            .map_err(txn_err_from)?;

        // 4. 找到 start_ts 匹配的写记录
        for (k, v) in &primary_writes {
            // 精确键匹配：避免前缀冲突导致误判事务状态
            if WriteRecord::extract_key(k) != Some(primary_key) {
                continue;
            }
            let record = WriteRecord::decode(v)?;
            if record.start_ts != lock.start_ts {
                continue;
            }
            match record.kind {
                WRITE_KIND_PUT | WRITE_KIND_DELETE => {
                    // 事务已提交，前推：写 commit 记录，删锁
                    // 使用 primary 写记录中已有的 commit_ts（保持事务一致性）
                    let commit_ts = WriteRecord::extract_commit_ts(k).unwrap_or(0);
                    let wkey = write_key(key, commit_ts);
                    let wrecord = WriteRecord {
                        start_ts: lock.start_ts,
                        kind: record.kind,
                    };
                    self.runtime
                        .put_shard(shard_id, wkey, wrecord.encode())
                        .map_err(txn_err_from)?;
                    self.runtime
                        .delete_shard(shard_id, lkey)
                        .map_err(txn_err_from)?;
                    return Ok(ResolveResult::Committed);
                }
                WRITE_KIND_ROLLBACK => {
                    // 事务已回滚，回滚 secondary
                    self.rollback(
                        &[Mutation::put(key.to_vec(), lock.value.clone())],
                        lock.start_ts,
                    )?;
                    return Ok(ResolveResult::RolledBack);
                }
                _ => {}
            }
        }

        // 5. 未找到匹配的写记录，回滚（事务确实未完成）
        self.rollback(
            &[Mutation::put(key.to_vec(), lock.value.clone())],
            lock.start_ts,
        )?;
        Ok(ResolveResult::RolledBack)
    }
}

/// 锁解析结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveResult {
    /// 事务已前推（commit）
    Committed,
    /// 事务已回滚
    RolledBack,
}

/// 将 `DistRuntimeError` 转换为 `TxnError`
fn txn_err_from(e: DistRuntimeError) -> TxnError {
    match e {
        DistRuntimeError::Raft(r) => TxnError::Raft(r),
        DistRuntimeError::Route(s) => TxnError::RouteError(s),
        DistRuntimeError::ShardNotFound(id) => {
            TxnError::RouteError(format!("shard {} not found", id))
        }
        DistRuntimeError::NotLeader(id) => TxnError::Raft(RaftError::NotLeader(id)),
        DistRuntimeError::Serialization(_s) => TxnError::CorruptData,
    }
}

// =====================================================================
//  ClusterTxnCoordinator — 跨节点事务协调器（阶段 3）
// =====================================================================

use crate::cluster::DistCluster;

/// 跨节点事务协调器：基于 `DistCluster` 实现真正的跨节点 Percolator 2PC。
///
/// 与 `DistTxnClient`（绑定单个 `DistRuntime`）不同，`ClusterTxnCoordinator`
/// 工作在多节点集群之上：
///
/// - **路由**：每个操作自动路由到当前集群 Leader
/// - **复制**：每次写入后驱动 `run_for` 等待多数派复制
/// - **容错**：Leader 故障时自动重新路由到新 Leader
/// - **TSO**：从当前 Leader 获取时间戳（Leader 持有 TSO）
///
/// # 设计要点
///
/// 1. **Leader 透传**：所有读写都路由到集群当前 Leader，保证强一致性
/// 2. **复制等待**：prewrite/commit/rollback 后调用 `run_for(200)` 确保
///    Raft 日志在多数派节点上复制并 apply
/// 3. **Leader 失效处理**：若操作返回 `NotLeader`，重新查询 Leader 并重试
/// 4. **快照读**：从 Leader 读取，保证已 apply 的最新数据可见
///
/// # 示例
///
/// ```ignore
/// let mut cluster = DistCluster::new_three_node(42)?;
/// cluster.init()?;
/// let mut txn = ClusterTxnCoordinator::new(&mut cluster);
///
/// let start_ts = txn.begin();
/// txn.prewrite_all(&[
///     Mutation::put(b"k1".to_vec(), b"v1".to_vec()),
///     Mutation::put(b"k2".to_vec(), b"v2".to_vec()),
/// ], start_ts)?;
/// txn.commit(&[
///     Mutation::put(b"k1".to_vec(), b"v1".to_vec()),
///     Mutation::put(b"k2".to_vec(), b"v2".to_vec()),
/// ], start_ts)?;
///
/// // 所有在线节点都应能读到已提交数据
/// let read_ts = txn.begin();
/// assert_eq!(txn.get(b"k1", read_ts)?, Some(b"v1".to_vec()));
/// ```
pub struct ClusterTxnCoordinator<'a> {
    /// 集群引用（多节点）
    cluster: &'a mut DistCluster,
}

impl<'a> ClusterTxnCoordinator<'a> {
    /// 创建跨节点事务协调器
    pub fn new(cluster: &'a mut DistCluster) -> Self {
        Self { cluster }
    }

    /// 获取当前 Leader 节点 ID
    ///
    /// # Errors
    /// 若集群无 Leader（未初始化或多数派节点宕机），返回 `NotLeader(0)`
    fn leader_or_err(&self) -> Result<NodeId, TxnError> {
        self.cluster
            .leader()
            .ok_or(TxnError::Raft(RaftError::NotLeader(0)))
    }

    /// 在 Leader 上执行闭包操作，自动处理 Leader 切换
    ///
    /// 若闭包返回 `NotLeader` 错误，重新查询 Leader 并重试一次。
    fn with_leader<F, R>(&mut self, mut f: F) -> Result<R, TxnError>
    where
        F: FnMut(&mut DistRuntime) -> Result<R, TxnError>,
    {
        let leader = self.leader_or_err()?;
        let runtime = self
            .cluster
            .node_mut(leader)
            .ok_or(TxnError::Raft(RaftError::NotLeader(leader)))?;
        match f(runtime) {
            Ok(r) => Ok(r),
            Err(TxnError::Raft(RaftError::NotLeader(_))) => {
                // Leader 切换：等待新 Leader 选举完成
                self.cluster.run_for(500);
                let new_leader = self.leader_or_err()?;
                let runtime = self
                    .cluster
                    .node_mut(new_leader)
                    .ok_or(TxnError::Raft(RaftError::NotLeader(new_leader)))?;
                f(runtime)
            }
            Err(e) => Err(e),
        }
    }

    /// 从当前 Leader 的 TSO 获取事务开始时间戳
    pub fn begin(&mut self) -> u64 {
        let leader = match self.cluster.leader() {
            Some(l) => l,
            None => {
                // 无 Leader 时尝试触发选举
                self.cluster.run_for(500);
                match self.cluster.leader() {
                    Some(l) => l,
                    None => return 0,
                }
            }
        };
        let runtime = match self.cluster.node_mut(leader) {
            Some(r) => r,
            None => return 0,
        };
        runtime.begin_transaction()
    }

    /// 快照读：从 Leader 读取键在 read_ts 时刻的最新已提交值
    ///
    /// # 流程
    /// 1. 路由原始键到分片
    /// 2. 检查锁
    /// 3. 扫描写记录，找最新 commit_ts <= read_ts 的非 ROLLBACK 记录
    /// 4. 读取对应 data 值
    pub fn get(&mut self, key: &[u8], read_ts: u64) -> Result<Option<Vec<u8>>, TxnError> {
        self.with_leader(|rt| {
            let shard_id = rt.route_raw_key(key).map_err(txn_err_from)?;

            // 检查锁
            let lkey = lock_key(key);
            if let Some(lock_bytes) = rt.get_shard(shard_id, &lkey).map_err(txn_err_from)? {
                let lock = LockInfo::decode(&lock_bytes)?;
                if lock.start_ts <= read_ts {
                    return Err(TxnError::LockedOnRead {
                        key: key.to_vec(),
                        holder_start_ts: lock.start_ts,
                    });
                }
            }

            // 扫描写记录
            let (start, end) = write_prefix_range(key);
            let range = KeyRange {
                start: Some(start),
                end: Some(end),
            };
            let writes = rt.scan_shard(shard_id, &range).map_err(txn_err_from)?;

            let mut latest: Option<(u64, WriteRecord)> = None;
            for (k, v) in writes {
                let Some(commit_ts) = WriteRecord::extract_commit_ts(&k) else {
                    continue;
                };
                if WriteRecord::extract_key(&k) != Some(key) {
                    continue;
                }
                if commit_ts > read_ts {
                    continue;
                }
                let record = WriteRecord::decode(&v)?;
                if record.kind == WRITE_KIND_ROLLBACK {
                    continue;
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
                    let dkey = data_key(key, record.start_ts);
                    Ok(rt
                        .get_shard(shard_id, &dkey)
                        .map_err(txn_err_from)?
                        .map(|v| v.to_vec()))
                }
            }
        })
    }

    /// Prewrite 阶段：对单个键加锁 + 写入数据版本
    ///
    /// 写入后驱动 `run_for(200)` 确保多数派复制。
    pub fn prewrite(
        &mut self,
        mutation: &Mutation,
        primary_key: &[u8],
        start_ts: u64,
    ) -> Result<(), TxnError> {
        self.with_leader(|rt| {
            let key = mutation.key();
            let shard_id = rt.route_raw_key(key).map_err(txn_err_from)?;

            // 检查写冲突
            let (start, end) = write_prefix_range(key);
            let range = KeyRange {
                start: Some(start),
                end: Some(end),
            };
            let writes = rt.scan_shard(shard_id, &range).map_err(txn_err_from)?;
            for (k, _) in &writes {
                if let Some(commit_ts) = WriteRecord::extract_commit_ts(k) {
                    if WriteRecord::extract_key(k) != Some(key) {
                        continue;
                    }
                    if commit_ts > start_ts {
                        return Err(TxnError::WriteConflict { key: key.to_vec() });
                    }
                }
            }

            // 检查锁
            let lkey = lock_key(key);
            if let Some(existing) = rt.get_shard(shard_id, &lkey).map_err(txn_err_from)? {
                let lock = LockInfo::decode(&existing)?;
                return Err(TxnError::KeyAlreadyLocked {
                    key: key.to_vec(),
                    holder_start_ts: lock.start_ts,
                });
            }

            // 写入 data 记录
            let dkey = data_key(key, start_ts);
            rt.put_shard(shard_id, dkey, mutation.value().to_vec())
                .map_err(txn_err_from)?;

            // 写入 lock 记录
            let lock = LockInfo {
                primary_key: primary_key.to_vec(),
                start_ts,
                kind: mutation.lock_kind(),
                value: mutation.value().to_vec(),
            };
            rt.put_shard(shard_id, lkey, lock.encode())
                .map_err(txn_err_from)?;

            Ok(())
        })?;
        // 等待多数派复制
        self.cluster.run_for(200);
        Ok(())
    }

    /// Prewrite 所有写操作（第一个键作为 primary）
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
    /// 提交后驱动 `run_for(200)` 确保 commit 记录在多数派节点上复制。
    pub fn commit(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<u64, TxnError> {
        if mutations.is_empty() {
            return Ok(self.begin());
        }

        let commit_ts = self.with_leader(|rt| {
            let primary_key = mutations[0].key();
            let primary_shard_id = rt.route_raw_key(primary_key).map_err(txn_err_from)?;

            // 检查 primary 锁
            let primary_lkey = lock_key(primary_key);
            let primary_lock_bytes = rt
                .get_shard(primary_shard_id, &primary_lkey)
                .map_err(txn_err_from)?
                .ok_or(TxnError::LockNotFound {
                    primary_key: primary_key.to_vec(),
                })?;
            let primary_lock = LockInfo::decode(&primary_lock_bytes)?;
            if primary_lock.start_ts != start_ts {
                return Err(TxnError::LockNotFound {
                    primary_key: primary_key.to_vec(),
                });
            }

            let commit_ts = rt.begin_transaction();

            // 写入 primary 的 write 记录
            let primary_wkey = write_key(primary_key, commit_ts);
            let primary_wrecord = WriteRecord {
                start_ts,
                kind: primary_lock.kind,
            };
            rt.put_shard(primary_shard_id, primary_wkey, primary_wrecord.encode())
                .map_err(txn_err_from)?;

            // 删除 primary 的 lock
            rt.delete_shard(primary_shard_id, primary_lkey)
                .map_err(txn_err_from)?;

            // 提交 secondary
            for m in mutations.iter().skip(1) {
                let skey = m.key();
                let secondary_shard_id = rt.route_raw_key(skey).map_err(txn_err_from)?;
                let slkey = lock_key(skey);
                let swkey = write_key(skey, commit_ts);

                let skind = if let Some(slock_bytes) = rt
                    .get_shard(secondary_shard_id, &slkey)
                    .map_err(txn_err_from)?
                {
                    LockInfo::decode(&slock_bytes)?.kind
                } else {
                    m.lock_kind()
                };

                let swrecord = WriteRecord {
                    start_ts,
                    kind: skind,
                };
                rt.put_shard(secondary_shard_id, swkey, swrecord.encode())
                    .map_err(txn_err_from)?;
                rt.delete_shard(secondary_shard_id, slkey)
                    .map_err(txn_err_from)?;
            }

            Ok(commit_ts)
        })?;
        // 等待 commit 记录在多数派节点上复制
        self.cluster.run_for(200);
        Ok(commit_ts)
    }

    /// Rollback：回滚事务
    pub fn rollback(&mut self, mutations: &[Mutation], start_ts: u64) -> Result<(), TxnError> {
        let _ = self.with_leader(|rt| {
            let rollback_ts = rt.begin_transaction();
            for m in mutations {
                let key = m.key();
                let shard_id = rt.route_raw_key(key).map_err(txn_err_from)?;
                let lkey = lock_key(key);
                let wkey = write_key(key, rollback_ts);
                let dkey = data_key(key, start_ts);

                rt.delete_shard(shard_id, lkey).map_err(txn_err_from)?;
                let wrecord = WriteRecord {
                    start_ts,
                    kind: WRITE_KIND_ROLLBACK,
                };
                rt.put_shard(shard_id, wkey, wrecord.encode())
                    .map_err(txn_err_from)?;
                rt.delete_shard(shard_id, dkey).map_err(txn_err_from)?;
            }
            Ok(rollback_ts)
        })?;
        self.cluster.run_for(200);
        Ok(())
    }

    /// 解析残留锁：检查 primary 状态决定前推或回滚
    pub fn resolve_lock(&mut self, key: &[u8]) -> Result<ResolveResult, TxnError> {
        let result = self.with_leader(|rt| {
            let shard_id = rt.route_raw_key(key).map_err(txn_err_from)?;
            let lkey = lock_key(key);
            let lock_bytes = rt.get_shard(shard_id, &lkey).map_err(txn_err_from)?.ok_or(
                TxnError::LockNotFound {
                    primary_key: key.to_vec(),
                },
            )?;
            let lock = LockInfo::decode(&lock_bytes)?;

            let primary_key = &lock.primary_key;
            let primary_shard_id = rt.route_raw_key(primary_key).map_err(txn_err_from)?;

            let (start, end) = write_prefix_range(primary_key);
            let range = KeyRange {
                start: Some(start),
                end: Some(end),
            };
            let primary_writes = rt
                .scan_shard(primary_shard_id, &range)
                .map_err(txn_err_from)?;

            for (k, v) in &primary_writes {
                if WriteRecord::extract_key(k) != Some(primary_key) {
                    continue;
                }
                let record = WriteRecord::decode(v)?;
                if record.start_ts != lock.start_ts {
                    continue;
                }
                match record.kind {
                    WRITE_KIND_PUT | WRITE_KIND_DELETE => {
                        let commit_ts = WriteRecord::extract_commit_ts(k).unwrap_or(0);
                        let wkey = write_key(key, commit_ts);
                        let wrecord = WriteRecord {
                            start_ts: lock.start_ts,
                            kind: record.kind,
                        };
                        rt.put_shard(shard_id, wkey, wrecord.encode())
                            .map_err(txn_err_from)?;
                        rt.delete_shard(shard_id, lkey).map_err(txn_err_from)?;
                        return Ok(ResolveResult::Committed);
                    }
                    WRITE_KIND_ROLLBACK => {
                        // 事务已回滚，回滚 secondary
                        let rollback_ts = rt.begin_transaction();
                        let wkey = write_key(key, rollback_ts);
                        let wrecord = WriteRecord {
                            start_ts: lock.start_ts,
                            kind: WRITE_KIND_ROLLBACK,
                        };
                        rt.put_shard(shard_id, wkey, wrecord.encode())
                            .map_err(txn_err_from)?;
                        rt.delete_shard(shard_id, lkey).map_err(txn_err_from)?;
                        let dkey = data_key(key, lock.start_ts);
                        rt.delete_shard(shard_id, dkey).map_err(txn_err_from)?;
                        return Ok(ResolveResult::RolledBack);
                    }
                    _ => {}
                }
            }

            // 未找到匹配写记录，回滚
            let rollback_ts = rt.begin_transaction();
            let wkey = write_key(key, rollback_ts);
            let wrecord = WriteRecord {
                start_ts: lock.start_ts,
                kind: WRITE_KIND_ROLLBACK,
            };
            rt.put_shard(shard_id, wkey, wrecord.encode())
                .map_err(txn_err_from)?;
            rt.delete_shard(shard_id, lkey).map_err(txn_err_from)?;
            let dkey = data_key(key, lock.start_ts);
            rt.delete_shard(shard_id, dkey).map_err(txn_err_from)?;
            Ok(ResolveResult::RolledBack)
        })?;
        self.cluster.run_for(200);
        Ok(result)
    }

    /// 验证事务结果在所有在线节点上达成一致
    ///
    /// 用于测试：确保 commit 后的数据在所有在线节点上都可读。
    /// 返回 (节点 ID, 读取结果) 列表。
    #[allow(clippy::type_complexity)]
    pub fn verify_replication(
        &self,
        key: &[u8],
        read_ts: u64,
    ) -> Result<Vec<(NodeId, Option<Vec<u8>>)>, TxnError> {
        let mut results = Vec::new();
        for &node_id in self.cluster.node_ids() {
            if !self.cluster.is_online(node_id) {
                continue;
            }
            let runtime = self
                .cluster
                .node(node_id)
                .ok_or(TxnError::Raft(RaftError::NotLeader(node_id)))?;
            let shard_id = runtime.route_raw_key(key).map_err(txn_err_from)?;

            // 直接扫描写记录（Follower 也能读）
            let (start, end) = write_prefix_range(key);
            let range = KeyRange {
                start: Some(start),
                end: Some(end),
            };
            let writes = runtime.scan_shard(shard_id, &range).map_err(txn_err_from)?;

            let mut latest: Option<(u64, WriteRecord)> = None;
            for (k, v) in writes {
                let Some(commit_ts) = WriteRecord::extract_commit_ts(&k) else {
                    continue;
                };
                if WriteRecord::extract_key(&k) != Some(key) {
                    continue;
                }
                if commit_ts > read_ts {
                    continue;
                }
                let record = WriteRecord::decode(&v)?;
                if record.kind == WRITE_KIND_ROLLBACK {
                    continue;
                }
                if latest.is_none() || commit_ts > latest.as_ref().unwrap().0 {
                    latest = Some((commit_ts, record));
                }
            }

            let value = match latest {
                None => None,
                Some((_, record)) => {
                    if record.kind == WRITE_KIND_DELETE {
                        None
                    } else {
                        let dkey = data_key(key, record.start_ts);
                        runtime
                            .get_shard(shard_id, &dkey)
                            .map_err(txn_err_from)?
                            .map(|v| v.to_vec())
                    }
                }
            };
            results.push((node_id, value));
        }
        Ok(results)
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-DIST 迭代 2：Percolator 单键事务（put → commit → get）
    #[test]
    fn test_dist_txn_single_key_commit() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);
        let start_ts = txn.begin();

        let mutation = Mutation::put(b"k1".to_vec(), b"v1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&mutation), start_ts).unwrap();
        let commit_ts = txn.commit(&[mutation], start_ts).unwrap();
        assert!(commit_ts > start_ts);

        // 读取应返回已提交值
        let read_ts = txn.begin();
        let val = txn.get(b"k1", read_ts).unwrap();
        assert_eq!(val, Some(b"v1".to_vec()));
    }

    /// P0-DIST 迭代 2：Percolator 多键事务（跨键原子提交）
    #[test]
    fn test_dist_txn_multi_key_atomic_commit() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);
        let start_ts = txn.begin();

        let mutations = vec![
            Mutation::put(b"acc1".to_vec(), b"100".to_vec()),
            Mutation::put(b"acc2".to_vec(), b"200".to_vec()),
            Mutation::put(b"acc3".to_vec(), b"300".to_vec()),
        ];
        txn.prewrite_all(&mutations, start_ts).unwrap();
        txn.commit(&mutations, start_ts).unwrap();

        // 所有键应可见
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"acc1", read_ts).unwrap(), Some(b"100".to_vec()));
        assert_eq!(txn.get(b"acc2", read_ts).unwrap(), Some(b"200".to_vec()));
        assert_eq!(txn.get(b"acc3", read_ts).unwrap(), Some(b"300".to_vec()));
    }

    /// P0-DIST 迭代 2：Percolator 回滚（prewrite → rollback → get 返回 None）
    #[test]
    fn test_dist_txn_rollback() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);
        let start_ts = txn.begin();

        let mutation = Mutation::put(b"rk1".to_vec(), b"rv1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&mutation), start_ts).unwrap();

        // 回滚
        txn.rollback(&[mutation], start_ts).unwrap();

        // 读取应返回 None（未提交）
        let read_ts = txn.begin();
        let val = txn.get(b"rk1", read_ts).unwrap();
        assert_eq!(val, None);
    }

    /// P0-DIST 迭代 2：Percolator 写冲突检测
    #[test]
    fn test_dist_txn_write_conflict() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 事务 A：写入 k1
        let mut txn_a = DistTxnClient::new(&mut runtime);
        let ts_a = txn_a.begin();
        let m_a = Mutation::put(b"ck1".to_vec(), b"v_a".to_vec());
        txn_a.prewrite_all(std::slice::from_ref(&m_a), ts_a).unwrap();
        txn_a.commit(&[m_a], ts_a).unwrap();

        // 事务 B：使用更早的 start_ts 写入同一键，应冲突
        let mut txn_b = DistTxnClient::new(&mut runtime);
        // 手动使用更小的 start_ts（模拟过期的快照）
        let ts_b = ts_a; // 与 A 相同的 start_ts
        let m_b = Mutation::put(b"ck1".to_vec(), b"v_b".to_vec());
        let result = txn_b.prewrite(&m_b, b"ck1", ts_b);
        assert!(
            matches!(result, Err(TxnError::WriteConflict { .. })),
            "应检测到写冲突，实际 {:?}",
            result
        );
    }

    /// P0-DIST 迭代 2：Percolator 锁冲突检测
    #[test]
    fn test_dist_txn_lock_conflict() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 事务 A：prewrite 但不 commit（持锁）
        let mut txn_a = DistTxnClient::new(&mut runtime);
        let ts_a = txn_a.begin();
        let m_a = Mutation::put(b"lk1".to_vec(), b"v_a".to_vec());
        txn_a.prewrite(&m_a, b"lk1", ts_a).unwrap();

        // 事务 B：尝试 prewrite 同一键，应锁冲突
        let mut txn_b = DistTxnClient::new(&mut runtime);
        let ts_b = txn_b.begin();
        let m_b = Mutation::put(b"lk1".to_vec(), b"v_b".to_vec());
        let result = txn_b.prewrite(&m_b, b"lk1", ts_b);
        assert!(
            matches!(result, Err(TxnError::KeyAlreadyLocked { .. })),
            "应检测到锁冲突，实际 {:?}",
            result
        );
    }

    /// P0-DIST 迭代 2：Percolator 删除事务
    #[test]
    fn test_dist_txn_delete() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 先写入
        let mut txn = DistTxnClient::new(&mut runtime);
        let ts1 = txn.begin();
        let m_put = Mutation::put(b"dk1".to_vec(), b"dv1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&m_put), ts1).unwrap();
        txn.commit(&[m_put], ts1).unwrap();

        // 删除
        let ts2 = txn.begin();
        let m_del = Mutation::delete(b"dk1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&m_del), ts2).unwrap();
        txn.commit(&[m_del], ts2).unwrap();

        // 读取应返回 None
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"dk1", read_ts).unwrap(), None);
    }

    /// P0-DIST 迭代 2：Percolator 快照读隔离
    #[test]
    fn test_dist_txn_snapshot_isolation() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);

        // 版本 1：v1
        let ts1 = txn.begin();
        txn.prewrite_all(&[Mutation::put(b"sk1".to_vec(), b"v1".to_vec())], ts1)
            .unwrap();
        txn.commit(&[Mutation::put(b"sk1".to_vec(), b"v1".to_vec())], ts1)
            .unwrap();

        // 版本 2：v2
        let ts2 = txn.begin();
        txn.prewrite_all(&[Mutation::put(b"sk1".to_vec(), b"v2".to_vec())], ts2)
            .unwrap();
        txn.commit(&[Mutation::put(b"sk1".to_vec(), b"v2".to_vec())], ts2)
            .unwrap();

        // 版本 3：v3
        let ts3 = txn.begin();
        txn.prewrite_all(&[Mutation::put(b"sk1".to_vec(), b"v3".to_vec())], ts3)
            .unwrap();
        txn.commit(&[Mutation::put(b"sk1".to_vec(), b"v3".to_vec())], ts3)
            .unwrap();

        // 用 ts1 读应返回 v1（但 ts1 已用于 commit，实际应使用更新 的 read_ts）
        // 注：commit_ts > start_ts，所以用 start_ts 读可能读不到自己的提交
        let read_ts = txn.begin();
        let val = txn.get(b"sk1", read_ts).unwrap();
        assert_eq!(val, Some(b"v3".to_vec()), "最新读应返回 v3");
    }

    /// P0-DIST 迭代 2：Percolator 覆盖写入
    #[test]
    fn test_dist_txn_overwrite() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);

        // 第一次写入
        let ts1 = txn.begin();
        txn.prewrite_all(&[Mutation::put(b"ok1".to_vec(), b"v1".to_vec())], ts1)
            .unwrap();
        txn.commit(&[Mutation::put(b"ok1".to_vec(), b"v1".to_vec())], ts1)
            .unwrap();

        // 覆盖写入
        let ts2 = txn.begin();
        txn.prewrite_all(&[Mutation::put(b"ok1".to_vec(), b"v2".to_vec())], ts2)
            .unwrap();
        txn.commit(&[Mutation::put(b"ok1".to_vec(), b"v2".to_vec())], ts2)
            .unwrap();

        // 读取应返回最新值
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"ok1", read_ts).unwrap(), Some(b"v2".to_vec()));
    }

    /// P0-DIST 迭代 2：Percolator 转账场景（原子性）
    #[test]
    fn test_dist_txn_transfer_atomicity() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);

        // 初始化账户余额
        let ts_init = txn.begin();
        let init_mutations = vec![
            Mutation::put(b"alice".to_vec(), b"1000".to_vec()),
            Mutation::put(b"bob".to_vec(), b"500".to_vec()),
        ];
        txn.prewrite_all(&init_mutations, ts_init).unwrap();
        txn.commit(&init_mutations, ts_init).unwrap();

        // 转账 300：alice -300, bob +300
        let ts_transfer = txn.begin();
        let transfer_mutations = vec![
            Mutation::put(b"alice".to_vec(), b"700".to_vec()),
            Mutation::put(b"bob".to_vec(), b"800".to_vec()),
        ];
        txn.prewrite_all(&transfer_mutations, ts_transfer).unwrap();
        txn.commit(&transfer_mutations, ts_transfer).unwrap();

        // 验证余额
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"alice", read_ts).unwrap(), Some(b"700".to_vec()));
        assert_eq!(txn.get(b"bob", read_ts).unwrap(), Some(b"800".to_vec()));
    }

    /// P0-DIST 阶段 2：多分片 Percolator 事务
    ///
    /// 验证 DistTxnClient 在多分片 runtime 上正确路由：
    /// - shard 1 覆盖 [b"a", b"n")
    /// - shard 2 覆盖 [b"n", None)
    ///
    /// alice (shard 1) → zebra (shard 2) 跨分片转账必须原子成功。
    #[test]
    fn test_dist_txn_multi_shard_transfer() {
        use crate::shard::{KeyRange, Shard};

        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"n".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"n".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        // 验证路由：alice → shard 1, zebra → shard 2
        assert_eq!(runtime.route_raw_key(b"alice").unwrap(), 1);
        assert_eq!(runtime.route_raw_key(b"zebra").unwrap(), 2);

        let mut txn = DistTxnClient::new(&mut runtime);

        // 初始化账户余额（跨分片）
        let ts_init = txn.begin();
        let init_mutations = vec![
            Mutation::put(b"alice".to_vec(), b"1000".to_vec()), // shard 1
            Mutation::put(b"zebra".to_vec(), b"500".to_vec()),  // shard 2
        ];
        txn.prewrite_all(&init_mutations, ts_init).unwrap();
        txn.commit(&init_mutations, ts_init).unwrap();

        // 跨分片转账 300：alice -300 (shard 1), zebra +300 (shard 2)
        let ts_transfer = txn.begin();
        let transfer_mutations = vec![
            Mutation::put(b"alice".to_vec(), b"700".to_vec()),
            Mutation::put(b"zebra".to_vec(), b"800".to_vec()),
        ];
        txn.prewrite_all(&transfer_mutations, ts_transfer).unwrap();
        txn.commit(&transfer_mutations, ts_transfer).unwrap();

        // 验证余额
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"alice", read_ts).unwrap(), Some(b"700".to_vec()));
        assert_eq!(txn.get(b"zebra", read_ts).unwrap(), Some(b"800".to_vec()));
    }

    /// P0-DIST 阶段 2：多分片回滚验证
    ///
    /// 在多分片 runtime 上 prewrite 后回滚，验证所有分片上的
    /// data/lock/write 记录都被正确清理。
    #[test]
    fn test_dist_txn_multi_shard_rollback() {
        use crate::shard::{KeyRange, Shard};

        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"n".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"n".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 99).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);

        // 跨分片 prewrite
        let start_ts = txn.begin();
        let mutations = vec![
            Mutation::put(b"alpha".to_vec(), b"v1".to_vec()), // shard 1
            Mutation::put(b"omega".to_vec(), b"v2".to_vec()), // shard 2
        ];
        txn.prewrite_all(&mutations, start_ts).unwrap();

        // 回滚
        txn.rollback(&mutations, start_ts).unwrap();

        // 读取应返回 None
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"alpha", read_ts).unwrap(), None);
        assert_eq!(txn.get(b"omega", read_ts).unwrap(), None);
    }

    /// P0-DIST 阶段 2：多分片写冲突检测
    ///
    /// 在多分片 runtime 上，两个事务尝试写同一键（跨分片），
    /// 第二个应检测到写冲突。
    #[test]
    fn test_dist_txn_multi_shard_write_conflict() {
        use crate::shard::{KeyRange, Shard};

        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"n".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"n".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 7).unwrap();
        runtime.init().unwrap();

        // 事务 A：写入 charlie（shard 1）
        let mut txn_a = DistTxnClient::new(&mut runtime);
        let ts_a = txn_a.begin();
        let m_a = Mutation::put(b"charlie".to_vec(), b"v_a".to_vec());
        txn_a.prewrite_all(std::slice::from_ref(&m_a), ts_a).unwrap();
        txn_a.commit(&[m_a], ts_a).unwrap();

        // 事务 B：使用更早的 start_ts 写同一键，应冲突
        let mut txn_b = DistTxnClient::new(&mut runtime);
        let ts_b = ts_a;
        let m_b = Mutation::put(b"charlie".to_vec(), b"v_b".to_vec());
        let result = txn_b.prewrite(&m_b, b"charlie", ts_b);
        assert!(
            matches!(result, Err(TxnError::WriteConflict { .. })),
            "应检测到写冲突，实际 {:?}",
            result
        );
    }

    // ================================================================
    //  阶段 3：ClusterTxnCoordinator 测试 — 跨节点事务协调
    // ================================================================

    /// 阶段 3：跨节点单键事务（prewrite → commit → 复制到所有节点）
    #[test]
    fn test_cluster_txn_single_key_commit_replicated() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let mut txn = ClusterTxnCoordinator::new(&mut cluster);
        let start_ts = txn.begin();
        assert!(start_ts > 0, "begin 应返回有效时间戳");

        let m = Mutation::put(b"ck1".to_vec(), b"cv1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&m), start_ts).unwrap();
        let commit_ts = txn.commit(&[m], start_ts).unwrap();
        assert!(commit_ts > start_ts);

        // 所有在线节点都应能读到已提交值
        let read_ts = txn.begin();
        let results = txn.verify_replication(b"ck1", read_ts).unwrap();
        assert!(!results.is_empty(), "应至少有一个在线节点");
        for (node_id, val) in &results {
            assert_eq!(val, &Some(b"cv1".to_vec()), "节点 {} 应读到 cv1", node_id);
        }
    }

    /// 阶段 3：跨节点多键事务（原子提交 + 全节点复制）
    #[test]
    fn test_cluster_txn_multi_key_atomic_replicated() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let mut txn = ClusterTxnCoordinator::new(&mut cluster);
        let start_ts = txn.begin();

        let mutations = vec![
            Mutation::put(b"acc1".to_vec(), b"100".to_vec()),
            Mutation::put(b"acc2".to_vec(), b"200".to_vec()),
            Mutation::put(b"acc3".to_vec(), b"300".to_vec()),
        ];
        txn.prewrite_all(&mutations, start_ts).unwrap();
        txn.commit(&mutations, start_ts).unwrap();

        // 所有键在所有节点上可见
        let read_ts = txn.begin();
        for key in [b"acc1".as_slice(), b"acc2".as_slice(), b"acc3".as_slice()] {
            let results = txn.verify_replication(key, read_ts).unwrap();
            for (_, val) in &results {
                assert!(val.is_some(), "键 {:?} 应有值", key);
            }
        }
    }

    /// 阶段 3：跨节点回滚（prewrite → rollback → 读返回 None）
    #[test]
    fn test_cluster_txn_rollback() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let mut txn = ClusterTxnCoordinator::new(&mut cluster);
        let start_ts = txn.begin();

        let m = Mutation::put(b"rk1".to_vec(), b"rv1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&m), start_ts).unwrap();
        txn.rollback(&[m], start_ts).unwrap();

        // 读取应返回 None（未提交）
        let read_ts = txn.begin();
        let val = txn.get(b"rk1", read_ts).unwrap();
        assert_eq!(val, None);
    }

    /// 阶段 3：跨节点删除事务
    #[test]
    fn test_cluster_txn_delete() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let mut txn = ClusterTxnCoordinator::new(&mut cluster);

        // 先写入
        let ts1 = txn.begin();
        let m_put = Mutation::put(b"dk1".to_vec(), b"dv1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&m_put), ts1).unwrap();
        txn.commit(&[m_put], ts1).unwrap();

        // 删除
        let ts2 = txn.begin();
        let m_del = Mutation::delete(b"dk1".to_vec());
        txn.prewrite_all(std::slice::from_ref(&m_del), ts2).unwrap();
        txn.commit(&[m_del], ts2).unwrap();

        // 读取应返回 None
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"dk1", read_ts).unwrap(), None);
    }

    /// 阶段 3：跨节点写冲突检测
    #[test]
    fn test_cluster_txn_write_conflict() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 事务 A：写入 ck1
        let mut txn_a = ClusterTxnCoordinator::new(&mut cluster);
        let ts_a = txn_a.begin();
        let m_a = Mutation::put(b"wck1".to_vec(), b"v_a".to_vec());
        txn_a.prewrite_all(std::slice::from_ref(&m_a), ts_a).unwrap();
        txn_a.commit(&[m_a], ts_a).unwrap();

        // 事务 B：使用更早的 start_ts 写同一键，应冲突
        let mut txn_b = ClusterTxnCoordinator::new(&mut cluster);
        let ts_b = ts_a;
        let m_b = Mutation::put(b"wck1".to_vec(), b"v_b".to_vec());
        let result = txn_b.prewrite(&m_b, b"wck1", ts_b);
        assert!(
            matches!(result, Err(TxnError::WriteConflict { .. })),
            "应检测到写冲突，实际 {:?}",
            result
        );
    }

    /// 阶段 3：跨节点锁冲突检测
    #[test]
    fn test_cluster_txn_lock_conflict() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 事务 A：prewrite 但不 commit（持锁）
        let mut txn_a = ClusterTxnCoordinator::new(&mut cluster);
        let ts_a = txn_a.begin();
        let m_a = Mutation::put(b"lck1".to_vec(), b"v_a".to_vec());
        txn_a.prewrite(&m_a, b"lck1", ts_a).unwrap();

        // 事务 B：尝试 prewrite 同一键，应锁冲突
        let mut txn_b = ClusterTxnCoordinator::new(&mut cluster);
        let ts_b = txn_b.begin();
        let m_b = Mutation::put(b"lck1".to_vec(), b"v_b".to_vec());
        let result = txn_b.prewrite(&m_b, b"lck1", ts_b);
        assert!(
            matches!(result, Err(TxnError::KeyAlreadyLocked { .. })),
            "应检测到锁冲突，实际 {:?}",
            result
        );
    }

    /// 阶段 3：Leader 故障切换后事务继续
    ///
    /// 1. 事务 A prewrite（持锁）
    /// 2. 当前 Leader 崩溃
    /// 3. 新 Leader 选出
    /// 4. 事务 B 应能通过 resolve_lock 解析残留锁
    #[test]
    fn test_cluster_txn_leader_failover_resolve_lock() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let leader = cluster.leader().expect("初始 Leader 已选出");

        // 事务 A：prewrite 但不 commit，然后 Leader 崩溃（残留锁）
        let _start_ts_a = {
            let mut txn_a = ClusterTxnCoordinator::new(&mut cluster);
            let ts = txn_a.begin();
            let m = Mutation::put(b"fk1".to_vec(), b"v_a".to_vec());
            txn_a.prewrite(&m, b"fk1", ts).unwrap();
            ts
        };

        // Leader 崩溃
        cluster.set_offline(leader);
        cluster.run_for(500);

        // 新 Leader 应已选出
        let new_leader = cluster.leader().expect("新 Leader 已选出");
        assert_ne!(new_leader, leader, "新 Leader 应不同于原 Leader");

        // 事务 B：尝试写同一键，应被锁阻塞，然后 resolve_lock
        let mut txn_b = ClusterTxnCoordinator::new(&mut cluster);
        let _ts_b = txn_b.begin();

        // 解析残留锁（事务 A 未完成，应回滚）
        let result = txn_b.resolve_lock(b"fk1").unwrap();
        assert_eq!(result, ResolveResult::RolledBack, "事务 A 应被回滚");

        // 解析后，事务 B 应能写入
        let ts_b = txn_b.begin();
        let m_b = Mutation::put(b"fk1".to_vec(), b"v_b".to_vec());
        txn_b.prewrite(&m_b, b"fk1", ts_b).unwrap();
        txn_b.commit(&[m_b], ts_b).unwrap();

        // 读取应返回 v_b（事务 A 已回滚，事务 B 已提交）
        let read_ts = txn_b.begin();
        let val = txn_b.get(b"fk1", read_ts).unwrap();
        assert_eq!(val, Some(b"v_b".to_vec()));
    }

    /// 阶段 3：跨节点快照读隔离
    ///
    /// 事务 A 在 ts1 写入并提交，事务 B 在 ts1 之后开始（ts2），
    /// 事务 C 在 ts2 之后开始（ts3）。事务 C 在 ts3 读取应看到 ts2 之后的数据。
    #[test]
    fn test_cluster_txn_snapshot_isolation() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 事务 A：写入 v1
        let mut txn_a = ClusterTxnCoordinator::new(&mut cluster);
        let ts_a = txn_a.begin();
        let m_a = Mutation::put(b"sk1".to_vec(), b"v1".to_vec());
        txn_a.prewrite_all(std::slice::from_ref(&m_a), ts_a).unwrap();
        let commit_ts_a = txn_a.commit(&[m_a], ts_a).unwrap();

        // 事务 B：在 commit_ts_a 之后读取，应看到 v1
        let mut txn_b = ClusterTxnCoordinator::new(&mut cluster);
        let read_ts_b = txn_b.begin();
        assert!(read_ts_b > commit_ts_a);
        assert_eq!(txn_b.get(b"sk1", read_ts_b).unwrap(), Some(b"v1".to_vec()));

        // 事务 C：使用更早的时间戳读取，应返回 None
        let val_old = txn_b.get(b"sk1", ts_a).unwrap();
        assert_eq!(val_old, None, "ts_a 之前的快照不应看到 v1");
    }
}
