//! 流复制 — Phase 7a.6
//!
//! # 设计
//!
//! - **`ReplicationPrimary`** — 主库，管理 WAL 记录流，接受备库连接并推送 WAL 批次
//! - **`ReplicationReplica`** — 备库，接收 WAL 批次并回放到本地页存储
//! - **`ReplicationMessage`** — 复制协议消息（WalBatch/Heartbeat/Eof）
//! - **`ReplicaStats`** — 复制统计（接收/应用/跳过记录数、字节数、批次数、心跳数、末尾 LSN）
//!
//! # 传输层
//!
//! 使用 tokio `unbounded_channel` 作为进程内传输（in-process channel）。
//! `UnboundedSender::send` 为同步操作，主库无需 `.await` 即可推送；
//! 备库通过 `recv().await` 异步接收。生产环境可替换为 TCP/TLS 传输。
//!
//! # 复制语义
//!
//! - **物理复制**：备库接收 Insert/Update/Delete/FullPageImage 记录，整页替换
//!   （与 `BackupManager::replay_wal` 语义一致）
//! - **增量流**：备库连接时指定 `start_lsn`，主库仅推送 `lsn > start_lsn` 的记录
//! - **批量化**：主库 `append_records` 一次推送一个批次，减少消息数
//! - **心跳**：主库无新记录时可发送 Heartbeat，备库更新末尾 LSN
//! - **优雅关闭**：主库发送 Eof，备库正常退出；主库崩溃（drop）时通道关闭，备库亦退出
//!
//! 对应 `SzRSQL实施进度.md` Phase 7a.6。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use szrsql_tx::wal::{WalOpType, WalRecord};
use thiserror::Error;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, instrument, trace, warn};

// =====================================================================
//  ReplicationError
// =====================================================================

/// 流复制错误类型
#[derive(Debug, Error)]
pub enum ReplicationError {
    /// I/O 错误
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 通道已关闭
    #[error("channel closed")]
    ChannelClosed,
    /// 无效 LSN（start_lsn > current_lsn）
    #[error("invalid start_lsn: {start_lsn} > current_lsn {current_lsn}")]
    InvalidLsn { start_lsn: u64, current_lsn: u64 },
    /// 备库 ID 为空
    #[error("replica id cannot be empty")]
    EmptyReplicaId,
    /// 备库已存在
    #[error("replica already exists: {0}")]
    ReplicaAlreadyExists(String),
    /// 备库不存在
    #[error("replica not found: {0}")]
    ReplicaNotFound(String),
}

// =====================================================================
//  ReplicationMessage — 复制协议消息
// =====================================================================

/// 复制协议消息（主库 → 备库）
#[derive(Debug, Clone)]
pub enum ReplicationMessage {
    /// WAL 记录批次
    WalBatch {
        /// 批次中的 WAL 记录
        records: Vec<WalRecord>,
        /// 批次起始 LSN
        start_lsn: u64,
        /// 批次结束 LSN
        end_lsn: u64,
    },
    /// 心跳（无新记录时发送，保持连接活跃）
    Heartbeat {
        /// 主库当前 LSN
        current_lsn: u64,
    },
    /// 流结束（优雅关闭）
    Eof,
}

// =====================================================================
//  ReplicaStats — 备库复制统计
// =====================================================================

/// 备库复制统计
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplicaStats {
    /// 接收的 WAL 记录总数
    pub records_received: u64,
    /// 已应用记录数（Insert/Update/Delete/FullPageImage）
    pub records_applied: u64,
    /// 已跳过记录数（Commit/Abort/Checkpoint）
    pub records_skipped: u64,
    /// 接收的批次数
    pub batches_received: u64,
    /// 接收的心跳数
    pub heartbeats_received: u64,
    /// 接收的字节总数（WalRecord::encoded_size 之和）
    pub bytes_received: u64,
    /// 末尾 LSN（最后一条记录的 LSN）
    pub last_lsn: u64,
    /// 更新已有页数
    pub pages_updated: u64,
    /// 新建页数
    pub pages_created: u64,
}

// =====================================================================
//  apply_records — WAL 记录回放辅助函数
// =====================================================================

/// 将 WAL 记录回放到页存储
///
/// 物理日志语义（与 `BackupManager::replay_wal` 一致）：
/// - `Insert` / `Update` / `Delete` / `FullPageImage`：`data` = 页后镜像，替换整页
/// - `Commit` / `Abort` / `Checkpoint`：不修改页，跳过
///
/// # 返回
/// `(records_applied, records_skipped, pages_updated, pages_created)`
pub fn apply_records(
    pages: &mut Vec<(u32, Vec<u8>)>,
    records: &[WalRecord],
) -> (u64, u64, u64, u64) {
    let mut page_index: HashMap<u32, usize> = HashMap::new();
    for (i, (page_id, _)) in pages.iter().enumerate() {
        page_index.insert(*page_id, i);
    }

    let mut records_applied = 0u64;
    let mut records_skipped = 0u64;
    let mut pages_updated = 0u64;
    let mut pages_created = 0u64;

    for record in records {
        match record.op_type {
            WalOpType::Insert
            | WalOpType::Update
            | WalOpType::Delete
            | WalOpType::FullPageImage
            | WalOpType::TableData => {
                if let Some(&idx) = page_index.get(&record.page_id) {
                    pages[idx].1 = record.data.clone();
                    pages_updated += 1;
                } else {
                    page_index.insert(record.page_id, pages.len());
                    pages.push((record.page_id, record.data.clone()));
                    pages_created += 1;
                }
                records_applied += 1;
            }
            WalOpType::Commit | WalOpType::Abort | WalOpType::Checkpoint => {
                records_skipped += 1;
            }
        }
    }

    (
        records_applied,
        records_skipped,
        pages_updated,
        pages_created,
    )
}

// =====================================================================
//  ReplicationPrimary — 主库
// =====================================================================

/// 主库（Primary）
///
/// 管理 WAL 记录流，接受备库连接并推送 WAL 批次。
///
/// # 示例
///
/// ```
/// use szrsql_replication::stream::{ReplicationPrimary, ReplicationReplica};
/// use szrsql_tx::wal::{WalRecord, WalOpType};
///
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// rt.block_on(async {
///     // 1. 创建主库
///     let primary = ReplicationPrimary::new("pri1");
///
///     // 2. 备库连接（start_lsn=0，从头开始）
///     let mut rx = primary.accept_replica("rep1", 0).unwrap();
///
///     // 3. 主库追加 WAL 记录
///     let records = vec![WalRecord::new(1, 1, WalOpType::FullPageImage, 0, vec![0xAA; 8192])];
///     primary.append_records(records);
///     primary.shutdown();
///
///     // 4. 备库接收并回放
///     let mut replica = ReplicationReplica::new("rep1", vec![(0u32, vec![0u8; 8192])]);
///     replica.run(&mut rx).await.unwrap();
///
///     // 5. 验证一致性
///     assert_eq!(replica.pages()[0].1, vec![0xAA; 8192]);
/// });
/// ```
pub struct ReplicationPrimary {
    /// 主库 ID
    primary_id: String,
    /// WAL 记录缓冲（全部历史）
    wal_records: Mutex<Vec<WalRecord>>,
    /// 当前 LSN（最后一条记录的 LSN，0 表示无记录）
    current_lsn: AtomicU64,
    /// 已连接备库的发送端
    replica_senders: Mutex<HashMap<String, UnboundedSender<ReplicationMessage>>>,
    /// 各备库已确认 LSN
    confirmed_lsns: Mutex<HashMap<String, u64>>,
}

impl ReplicationPrimary {
    /// 创建主库
    pub fn new(primary_id: &str) -> Self {
        Self {
            primary_id: primary_id.to_string(),
            wal_records: Mutex::new(Vec::new()),
            current_lsn: AtomicU64::new(0),
            replica_senders: Mutex::new(HashMap::new()),
            confirmed_lsns: Mutex::new(HashMap::new()),
        }
    }

    /// 主库 ID
    pub fn primary_id(&self) -> &str {
        &self.primary_id
    }

    /// 当前 LSN
    pub fn current_lsn(&self) -> u64 {
        self.current_lsn.load(Ordering::SeqCst)
    }

    /// WAL 记录总数
    pub fn record_count(&self) -> usize {
        self.wal_records.lock().unwrap().len()
    }

    /// 已连接备库数
    pub fn replica_count(&self) -> usize {
        self.replica_senders.lock().unwrap().len()
    }

    /// 获取指定备库的已确认 LSN
    pub fn confirmed_lsn(&self, replica_id: &str) -> Option<u64> {
        self.confirmed_lsns.lock().unwrap().get(replica_id).copied()
    }

    /// 接受备库连接
    ///
    /// 创建新的无界通道，注册备库发送端，并立即推送 `lsn > start_lsn` 的存量记录作为追平批次。
    ///
    /// # 参数
    /// - `replica_id` — 备库 ID
    /// - `start_lsn` — 备库请求从此 LSN 之后开始接收（0 表示从头）
    ///
    /// # 返回
    /// 备库的消息接收端 `UnboundedReceiver<ReplicationMessage>`
    #[instrument(skip(self), fields(replica_id = %replica_id, start_lsn))]
    pub fn accept_replica(
        &self,
        replica_id: &str,
        start_lsn: u64,
    ) -> Result<UnboundedReceiver<ReplicationMessage>, ReplicationError> {
        if replica_id.is_empty() {
            return Err(ReplicationError::EmptyReplicaId);
        }

        let (tx, rx) = mpsc::unbounded_channel();

        // 注册备库
        {
            let mut senders = self.replica_senders.lock().unwrap();
            if senders.contains_key(replica_id) {
                return Err(ReplicationError::ReplicaAlreadyExists(
                    replica_id.to_string(),
                ));
            }
            senders.insert(replica_id.to_string(), tx);
        }
        {
            let mut confirmed = self.confirmed_lsns.lock().unwrap();
            confirmed.insert(replica_id.to_string(), 0);
        }

        debug!(
            replica_id,
            start_lsn,
            current_lsn = self.current_lsn.load(Ordering::SeqCst),
            "replica connected, sending catchup batch"
        );

        // 推送存量记录（追平批次）
        let records: Vec<WalRecord> = {
            let wal = self.wal_records.lock().unwrap();
            wal.iter().filter(|r| r.lsn > start_lsn).cloned().collect()
        };
        if !records.is_empty() {
            let start = records.first().unwrap().lsn;
            let end = records.last().unwrap().lsn;
            trace!(
                replica_id,
                batch_size = records.len(),
                start_lsn = start,
                end_lsn = end,
                "catchup batch pushed"
            );
            self.send_to_replica(
                replica_id,
                ReplicationMessage::WalBatch {
                    records,
                    start_lsn: start,
                    end_lsn: end,
                },
            );
        }

        Ok(rx)
    }

    /// 追加 WAL 记录并推送到所有已连接备库
    ///
    /// # 返回
    /// 追加后的新当前 LSN
    pub fn append_records(&self, records: Vec<WalRecord>) -> u64 {
        if records.is_empty() {
            return self.current_lsn.load(Ordering::SeqCst);
        }

        let start_lsn = records.first().unwrap().lsn;
        let end_lsn = records.last().unwrap().lsn;
        let batch_size = records.len();

        // 追加到 WAL 缓冲
        {
            let mut wal = self.wal_records.lock().unwrap();
            wal.extend(records.clone());
        }
        self.current_lsn.store(end_lsn, Ordering::SeqCst);

        debug!(
            batch_size,
            start_lsn,
            end_lsn,
            replica_count = self.replica_senders.lock().unwrap().len(),
            "appended WAL batch and fanning out to replicas"
        );

        // 扇出到所有备库
        let msg = ReplicationMessage::WalBatch {
            records,
            start_lsn,
            end_lsn,
        };
        let senders = self.replica_senders.lock().unwrap();
        for tx in senders.values() {
            let _ = tx.send(msg.clone());
        }

        end_lsn
    }

    /// 发送心跳到所有备库
    pub fn send_heartbeat(&self) {
        let current = self.current_lsn.load(Ordering::SeqCst);
        let msg = ReplicationMessage::Heartbeat {
            current_lsn: current,
        };
        let senders = self.replica_senders.lock().unwrap();
        let replica_count = senders.len();
        for tx in senders.values() {
            let _ = tx.send(msg.clone());
        }
        trace!(current_lsn = current, replica_count, "heartbeat sent");
    }

    /// 优雅关闭：向所有备库发送 Eof
    pub fn shutdown(&self) {
        let senders = self.replica_senders.lock().unwrap();
        let replica_count = senders.len();
        for tx in senders.values() {
            let _ = tx.send(ReplicationMessage::Eof);
        }
        info!(replica_count, "primary graceful shutdown, Eof sent to all replicas");
    }

    /// 模拟主库崩溃：关闭所有备库通道（不发 Eof），备库 recv 返回 None 后退出
    ///
    /// 与 `shutdown` 的区别：`shutdown` 发送 Eof（优雅关闭），`crash` 直接关闭通道（模拟崩溃）。
    /// 主库 WAL 数据保留，可用于 `expected_pages` 一致性校验。
    pub fn crash(&self) {
        let mut senders = self.replica_senders.lock().unwrap();
        let replica_count = senders.len();
        senders.clear();
        warn!(replica_count, "primary crash simulated, channels closed without Eof");
    }

    /// 移除备库连接
    pub fn remove_replica(&self, replica_id: &str) -> Result<(), ReplicationError> {
        let mut senders = self.replica_senders.lock().unwrap();
        if senders.remove(replica_id).is_none() {
            return Err(ReplicationError::ReplicaNotFound(replica_id.to_string()));
        }
        self.confirmed_lsns.lock().unwrap().remove(replica_id);
        Ok(())
    }

    /// 更新备库已确认 LSN（由备库通过外部机制反馈，如 RPC 回调）
    pub fn update_confirmed_lsn(&self, replica_id: &str, lsn: u64) {
        let mut confirmed = self.confirmed_lsns.lock().unwrap();
        confirmed.insert(replica_id.to_string(), lsn);
    }

    /// 计算主库期望的最终页状态（将全部 WAL 回放到初始页）
    ///
    /// 用于一致性校验：备库最终页状态应与此方法结果一致。
    pub fn expected_pages(&self, initial_pages: &[(u32, Vec<u8>)]) -> Vec<(u32, Vec<u8>)> {
        let mut pages = initial_pages.to_vec();
        let wal = self.wal_records.lock().unwrap();
        apply_records(&mut pages, &wal);
        pages
    }

    /// 向指定备库发送消息
    fn send_to_replica(&self, replica_id: &str, msg: ReplicationMessage) {
        let senders = self.replica_senders.lock().unwrap();
        if let Some(tx) = senders.get(replica_id) {
            let _ = tx.send(msg);
        }
    }
}

// =====================================================================
//  ReplicationReplica — 备库
// =====================================================================

/// 备库（Replica）
///
/// 接收主库推送的 WAL 批次并回放到本地页存储。
pub struct ReplicationReplica {
    /// 备库 ID
    replica_id: String,
    /// 本地页存储
    pages: Vec<(u32, Vec<u8>)>,
    /// 已确认 LSN（最后一条已应用记录的 LSN）
    confirmed_lsn: u64,
    /// 复制统计
    stats: ReplicaStats,
}

impl ReplicationReplica {
    /// 创建备库
    ///
    /// # 参数
    /// - `replica_id` — 备库 ID
    /// - `initial_pages` — 初始页状态（通常从全量备份恢复）
    pub fn new(replica_id: &str, initial_pages: Vec<(u32, Vec<u8>)>) -> Self {
        Self {
            replica_id: replica_id.to_string(),
            pages: initial_pages,
            confirmed_lsn: 0,
            stats: ReplicaStats::default(),
        }
    }

    /// 备库 ID
    pub fn replica_id(&self) -> &str {
        &self.replica_id
    }

    /// 本地页存储
    pub fn pages(&self) -> &[(u32, Vec<u8>)] {
        &self.pages
    }

    /// 已确认 LSN
    pub fn confirmed_lsn(&self) -> u64 {
        self.confirmed_lsn
    }

    /// 复制统计
    pub fn stats(&self) -> &ReplicaStats {
        &self.stats
    }

    /// 运行备库接收循环
    ///
    /// 从通道接收消息，应用 WAL 批次到本地页存储，直到收到 Eof 或通道关闭。
    ///
    /// # 返回
    /// 复制统计的克隆
    pub async fn run(
        &mut self,
        receiver: &mut UnboundedReceiver<ReplicationMessage>,
    ) -> Result<ReplicaStats, ReplicationError> {
        while let Some(msg) = receiver.recv().await {
            match msg {
                ReplicationMessage::WalBatch {
                    records,
                    start_lsn: _,
                    end_lsn,
                } => {
                    self.stats.batches_received += 1;
                    self.stats.records_received += records.len() as u64;
                    for r in &records {
                        self.stats.bytes_received += r.encoded_size() as u64;
                    }

                    let (applied, skipped, p_updated, p_created) =
                        apply_records(&mut self.pages, &records);
                    self.stats.records_applied += applied;
                    self.stats.records_skipped += skipped;
                    self.stats.pages_updated += p_updated;
                    self.stats.pages_created += p_created;

                    self.confirmed_lsn = end_lsn;
                    self.stats.last_lsn = end_lsn;
                }
                ReplicationMessage::Heartbeat { current_lsn } => {
                    self.stats.heartbeats_received += 1;
                    self.stats.last_lsn = current_lsn;
                }
                ReplicationMessage::Eof => {
                    break;
                }
            }
        }
        // 循环退出：收到 Eof（优雅关闭）或通道关闭（主库崩溃）
        Ok(self.stats.clone())
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const PAGE_SIZE: usize = 8192;

    /// 生成测试页数据：page_id → [page_id as u8; PAGE_SIZE]
    fn make_pages(count: u32) -> Vec<(u32, Vec<u8>)> {
        (0..count).map(|i| (i, vec![i as u8; PAGE_SIZE])).collect()
    }

    /// 生成 FullPageImage WalRecord 列表
    fn make_wal_records(start_lsn: u64, page_count: u32, value: u8) -> Vec<WalRecord> {
        (0..page_count)
            .map(|i| {
                let mut record = WalRecord::new(
                    start_lsn + i as u64,
                    1,
                    WalOpType::FullPageImage,
                    i,
                    vec![value; PAGE_SIZE],
                );
                record.update_checksum();
                record
            })
            .collect()
    }

    /// 生成 Commit 记录（应被跳过）
    fn make_commit_records(start_lsn: u64, count: u64) -> Vec<WalRecord> {
        (0..count)
            .map(|i| {
                let mut record = WalRecord::new(start_lsn + i, 1, WalOpType::Commit, 0, vec![]);
                record.update_checksum();
                record
            })
            .collect()
    }

    // -----------------------------------------------------------------
    //  基础测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_basic_stream_replication() {
        // 主库追加 10 条 FullPageImage 记录，备库接收并回放
        let primary = ReplicationPrimary::new("pri_basic");
        let mut rx = primary.accept_replica("rep_basic", 0).unwrap();

        let records = make_wal_records(1, 10, 0xBB);
        primary.append_records(records);
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_basic", make_pages(10));
        replica.run(&mut rx).await.unwrap();

        // 验证：10 页全部被更新为 0xBB
        assert_eq!(replica.pages().len(), 10);
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![0xBB; PAGE_SIZE]);
        }
        assert_eq!(replica.confirmed_lsn(), 10);
        assert_eq!(replica.stats().records_received, 10);
        assert_eq!(replica.stats().records_applied, 10);
        assert_eq!(replica.stats().records_skipped, 0);
        assert_eq!(replica.stats().batches_received, 1);
    }

    #[tokio::test]
    async fn test_7a6_empty_wal() {
        // 主库无 WAL 记录，备库只收到 Eof
        let primary = ReplicationPrimary::new("pri_empty");
        let mut rx = primary.accept_replica("rep_empty", 0).unwrap();

        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_empty", make_pages(5));
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.stats().records_received, 0);
        assert_eq!(replica.stats().batches_received, 0);
        assert_eq!(replica.confirmed_lsn(), 0);
    }

    #[tokio::test]
    async fn test_7a6_eof_graceful_shutdown() {
        // 主库发送数据后 Eof，备库正常退出
        let primary = ReplicationPrimary::new("pri_eof");
        let mut rx = primary.accept_replica("rep_eof", 0).unwrap();

        primary.append_records(make_wal_records(1, 5, 0xCC));
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_eof", make_pages(5));
        let stats = replica.run(&mut rx).await.unwrap();

        assert_eq!(stats.records_applied, 5);
        assert_eq!(replica.confirmed_lsn(), 5);
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![0xCC; PAGE_SIZE]);
        }
    }

    // -----------------------------------------------------------------
    //  增量复制测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_incremental_replication() {
        // 备库从 start_lsn=5 开始，仅接收 lsn > 5 的记录
        let primary = ReplicationPrimary::new("pri_incr");

        // 先追加 10 条记录（lsn 1-10）
        primary.append_records(make_wal_records(1, 10, 0xAA));

        // 备库从 lsn=5 开始连接 → 接收 lsn 6-10 共 5 条
        let mut rx = primary.accept_replica("rep_incr", 5).unwrap();
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_incr", make_pages(10));
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.stats().records_received, 5);
        assert_eq!(replica.confirmed_lsn(), 10);
    }

    #[tokio::test]
    async fn test_7a6_catchup_on_connect() {
        // 主库先有 5 条记录，备库连接后立即收到追平批次
        let primary = ReplicationPrimary::new("pri_catchup");
        primary.append_records(make_wal_records(1, 5, 0xDD));

        let mut rx = primary.accept_replica("rep_catchup", 0).unwrap();
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_catchup", make_pages(5));
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.stats().records_received, 5);
        assert_eq!(replica.confirmed_lsn(), 5);
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![0xDD; PAGE_SIZE]);
        }
    }

    // -----------------------------------------------------------------
    //  多批次 + 一致性测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_multi_batch_consistency() {
        // 主库分 5 批次推送 50 条记录，备库最终状态与主库一致
        let primary = ReplicationPrimary::new("pri_multi");
        let mut rx = primary.accept_replica("rep_multi", 0).unwrap();

        // 5 批次 × 10 条 = 50 条，每批次更新 10 页
        for batch in 0..5u8 {
            let start_lsn = (batch as u64) * 10 + 1;
            primary.append_records(make_wal_records(start_lsn, 10, 0x10 + batch));
        }
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_multi", make_pages(10));
        replica.run(&mut rx).await.unwrap();

        // 验证：最后一批次（batch=4）的值 0x14 覆盖所有页
        assert_eq!(replica.stats().batches_received, 5);
        assert_eq!(replica.stats().records_received, 50);
        assert_eq!(replica.confirmed_lsn(), 50);
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![0x14; PAGE_SIZE]);
        }

        // 与主库期望状态一致
        let expected = primary.expected_pages(&make_pages(10));
        assert_eq!(replica.pages(), expected.as_slice());
    }

    // -----------------------------------------------------------------
    //  多副本测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_multi_replica() {
        // 2 个备库同时连接，都收到相同的 WAL 流
        let primary = ReplicationPrimary::new("pri_mrep");
        let mut rx1 = primary.accept_replica("rep1", 0).unwrap();
        let mut rx2 = primary.accept_replica("rep2", 0).unwrap();

        assert_eq!(primary.replica_count(), 2);

        primary.append_records(make_wal_records(1, 10, 0xEE));
        primary.shutdown();

        let init_pages = make_pages(10);
        let mut replica1 = ReplicationReplica::new("rep1", init_pages.clone());
        let mut replica2 = ReplicationReplica::new("rep2", init_pages.clone());

        // 并发运行两个备库
        let (stats1, stats2) = tokio::join!(replica1.run(&mut rx1), replica2.run(&mut rx2),);

        stats1.unwrap();
        stats2.unwrap();

        // 两个备库状态一致
        assert_eq!(replica1.pages(), replica2.pages());
        assert_eq!(replica1.confirmed_lsn(), replica2.confirmed_lsn());
        assert_eq!(replica1.confirmed_lsn(), 10);

        for (_, page_bytes) in replica1.pages() {
            assert_eq!(page_bytes, &vec![0xEE; PAGE_SIZE]);
        }
    }

    // -----------------------------------------------------------------
    //  主库崩溃测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_primary_crash() {
        // 主库发送 1000 条记录后崩溃（crash），备库数据完整
        let primary = ReplicationPrimary::new("pri_crash");
        let mut rx = primary.accept_replica("rep_crash", 0).unwrap();

        // 追加 1000 条记录（100 页 × 10 次/页）
        for batch in 0..10u8 {
            let start_lsn = (batch as u64) * 100 + 1;
            primary.append_records(make_wal_records(start_lsn, 100, 0xF0 + batch));
        }

        // 模拟主库崩溃：关闭通道（不发 Eof），备库 recv 返回 None 后退出
        // WAL 数据保留在 primary 中，用于 expected_pages 一致性校验
        primary.crash();

        let mut replica = ReplicationReplica::new("rep_crash", make_pages(100));
        replica.run(&mut rx).await.unwrap();

        // 验证：1000 条全部接收
        assert_eq!(replica.stats().records_received, 1000);
        assert_eq!(replica.confirmed_lsn(), 1000);

        // 验证数据完整：与主库期望状态一致
        let expected = primary.expected_pages(&make_pages(100));
        assert_eq!(replica.pages(), expected.as_slice());

        // 最后一批次（batch=9）的值 0xF9 覆盖所有页
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![0xF9; PAGE_SIZE]);
        }
    }

    // -----------------------------------------------------------------
    //  心跳测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_heartbeat() {
        let primary = ReplicationPrimary::new("pri_hb");
        let mut rx = primary.accept_replica("rep_hb", 0).unwrap();

        // 追加 5 条记录
        primary.append_records(make_wal_records(1, 5, 0x11));
        // 发送心跳（current_lsn=5）
        primary.send_heartbeat();
        // 优雅关闭
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_hb", make_pages(5));
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.stats().records_received, 5);
        assert_eq!(replica.stats().heartbeats_received, 1);
        assert_eq!(replica.stats().last_lsn, 5);
        assert_eq!(replica.confirmed_lsn(), 5);
    }

    // -----------------------------------------------------------------
    //  大批次测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_large_batch() {
        // 单批次 10000 条记录
        let primary = ReplicationPrimary::new("pri_large");
        let mut rx = primary.accept_replica("rep_large", 0).unwrap();

        // 1000 页，每页更新 10 次 = 10000 条记录
        let records: Vec<WalRecord> = (0..10000u64)
            .map(|i| {
                let page_id = (i % 1000) as u32;
                let value = (i / 1000) as u8; // 第 0 轮=0, 第 1 轮=1, ..., 第 9 轮=9
                let mut record = WalRecord::new(
                    i + 1,
                    1,
                    WalOpType::FullPageImage,
                    page_id,
                    vec![value; PAGE_SIZE],
                );
                record.update_checksum();
                record
            })
            .collect();
        primary.append_records(records);
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_large", make_pages(1000));
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.stats().records_received, 10000);
        assert_eq!(replica.stats().batches_received, 1);
        assert_eq!(replica.confirmed_lsn(), 10000);

        // 每页最终值 = 9（第 9 轮，最后一次更新）
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![9u8; PAGE_SIZE]);
        }

        let expected = primary.expected_pages(&make_pages(1000));
        assert_eq!(replica.pages(), expected.as_slice());
    }

    // -----------------------------------------------------------------
    //  跳过记录测试（Commit/Abort/Checkpoint）
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_skip_control_records() {
        let primary = ReplicationPrimary::new("pri_skip");
        let mut rx = primary.accept_replica("rep_skip", 0).unwrap();

        // 混合：5 条 FullPageImage + 3 条 Commit + 2 条 FullPageImage
        let mut records = make_wal_records(1, 5, 0x22);
        records.extend(make_commit_records(6, 3));
        records.extend(make_wal_records(9, 2, 0x33));
        primary.append_records(records);
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_skip", make_pages(5));
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.stats().records_received, 10);
        assert_eq!(replica.stats().records_applied, 7); // 5 + 2
        assert_eq!(replica.stats().records_skipped, 3); // 3 Commit
        assert_eq!(replica.confirmed_lsn(), 10);
    }

    // -----------------------------------------------------------------
    //  新页创建测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_new_page_creation() {
        // 备库初始 0 页，接收 5 条 FullPageImage → 创建 5 个新页
        let primary = ReplicationPrimary::new("pri_new");
        let mut rx = primary.accept_replica("rep_new", 0).unwrap();

        primary.append_records(make_wal_records(1, 5, 0x44));
        primary.shutdown();

        let mut replica = ReplicationReplica::new("rep_new", vec![]);
        replica.run(&mut rx).await.unwrap();

        assert_eq!(replica.pages().len(), 5);
        assert_eq!(replica.stats().pages_created, 5);
        assert_eq!(replica.stats().pages_updated, 0);
        for (_, page_bytes) in replica.pages() {
            assert_eq!(page_bytes, &vec![0x44; PAGE_SIZE]);
        }
    }

    // -----------------------------------------------------------------
    //  集成测试：1M 行流复制
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_integration_1m_rows() {
        // 模拟主库 INSERT 1000000 行 → 备库 WAL 流式接收 → 备库回放 → 数据一致
        //
        // 1M 行 × 8 字节/行 = 8MB，按 1024 行/页 = 977 页 × 8KB/页
        // 每个 FullPageImage 记录 = 1 页 = 8KB
        // 977 条 WalRecord，分 10 批次推送（每批 ~98 条）

        const ROW_SIZE: usize = 8; // 8 字节/行
        const ROWS_PER_PAGE: usize = PAGE_SIZE / ROW_SIZE; // 1024 行/页
        const TOTAL_ROWS: usize = 1_000_000;
        const TOTAL_PAGES: usize = TOTAL_ROWS.div_ceil(ROWS_PER_PAGE); // 977 页
        const BATCH_COUNT: usize = 10;

        let start = Instant::now();

        // 1. 创建主库 + 备库连接
        let primary = ReplicationPrimary::new("pri_1m");
        let mut rx = primary.accept_replica("rep_1m", 0).unwrap();

        // 2. 生成 977 页的 FullPageImage 记录，分 10 批推送
        let pages_per_batch = TOTAL_PAGES.div_ceil(BATCH_COUNT); // ~98
        for batch in 0..BATCH_COUNT {
            let start_page = batch * pages_per_batch;
            let end_page = ((batch + 1) * pages_per_batch).min(TOTAL_PAGES);
            if start_page >= end_page {
                break;
            }

            // 每页内容 = [page_id as u8; PAGE_SIZE]（模拟 1024 行数据）
            let records: Vec<WalRecord> = (start_page..end_page)
                .map(|page_id| {
                    let lsn = (page_id + 1) as u64;
                    let mut record = WalRecord::new(
                        lsn,
                        1,
                        WalOpType::FullPageImage,
                        page_id as u32,
                        vec![page_id as u8; PAGE_SIZE],
                    );
                    record.update_checksum();
                    record
                })
                .collect();
            primary.append_records(records);
        }
        primary.shutdown();

        // 3. 备库接收并回放
        let initial_pages: Vec<(u32, Vec<u8>)> = (0..TOTAL_PAGES as u32)
            .map(|i| (i, vec![0u8; PAGE_SIZE]))
            .collect();
        let mut replica = ReplicationReplica::new("rep_1m", initial_pages.clone());
        replica.run(&mut rx).await.unwrap();

        let elapsed = start.elapsed();

        // 4. 验证一致性
        assert_eq!(replica.stats().records_received, TOTAL_PAGES as u64);
        assert_eq!(replica.confirmed_lsn(), TOTAL_PAGES as u64);

        // 逐页校验：每页内容 = [page_id as u8; PAGE_SIZE]
        for (page_id, page_bytes) in replica.pages() {
            assert_eq!(
                page_bytes,
                &vec![*page_id as u8; PAGE_SIZE],
                "page {} content mismatch",
                page_id
            );
        }

        // 与主库期望状态一致
        let expected = primary.expected_pages(&initial_pages);
        assert_eq!(replica.pages(), expected.as_slice());

        // 5. 验证延迟 < 1s（977 页 × 8KB = ~8MB 通过通道 + 回放）
        assert!(
            elapsed.as_secs() < 1,
            "replication lag {}ms >= 1000ms",
            elapsed.as_millis()
        );

        println!(
            "1M rows ({} pages, ~8MB) replicated in {}ms, lag < 1s ✅",
            TOTAL_PAGES,
            elapsed.as_millis()
        );
    }

    // -----------------------------------------------------------------
    //  错误处理测试
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_7a6_empty_replica_id() {
        let primary = ReplicationPrimary::new("pri_err");
        let result = primary.accept_replica("", 0);
        assert!(matches!(result, Err(ReplicationError::EmptyReplicaId)));
    }

    #[tokio::test]
    async fn test_7a6_duplicate_replica() {
        let primary = ReplicationPrimary::new("pri_dup");
        let _rx1 = primary.accept_replica("rep_dup", 0).unwrap();
        let result = primary.accept_replica("rep_dup", 0);
        assert!(matches!(
            result,
            Err(ReplicationError::ReplicaAlreadyExists(_))
        ));
    }

    #[tokio::test]
    async fn test_7a6_remove_replica() {
        let primary = ReplicationPrimary::new("pri_rm");
        let _rx = primary.accept_replica("rep_rm", 0).unwrap();
        assert_eq!(primary.replica_count(), 1);

        primary.remove_replica("rep_rm").unwrap();
        assert_eq!(primary.replica_count(), 0);

        // 二次移除报错
        let result = primary.remove_replica("rep_rm");
        assert!(matches!(result, Err(ReplicationError::ReplicaNotFound(_))));
    }

    #[tokio::test]
    async fn test_7a6_confirmed_lsn_tracking() {
        let primary = ReplicationPrimary::new("pri_conf");
        let _rx = primary.accept_replica("rep_conf", 0).unwrap();

        primary.update_confirmed_lsn("rep_conf", 100);
        assert_eq!(primary.confirmed_lsn("rep_conf"), Some(100));

        primary.update_confirmed_lsn("rep_conf", 200);
        assert_eq!(primary.confirmed_lsn("rep_conf"), Some(200));

        // 不存在的备库
        assert_eq!(primary.confirmed_lsn("nonexistent"), None);
    }

    // -----------------------------------------------------------------
    //  apply_records 单元测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7a6_apply_records_basic() {
        let mut pages = vec![(0u32, vec![0u8; PAGE_SIZE])];
        let records = make_wal_records(1, 1, 0xAB);
        let (applied, skipped, updated, created) = apply_records(&mut pages, &records);

        assert_eq!(applied, 1);
        assert_eq!(skipped, 0);
        assert_eq!(updated, 1);
        assert_eq!(created, 0);
        assert_eq!(pages[0].1, vec![0xAB; PAGE_SIZE]);
    }

    #[test]
    fn test_7a6_apply_records_new_page() {
        let mut pages: Vec<(u32, Vec<u8>)> = vec![];
        let records = make_wal_records(1, 3, 0xCD);
        let (applied, skipped, updated, created) = apply_records(&mut pages, &records);

        assert_eq!(applied, 3);
        assert_eq!(skipped, 0);
        assert_eq!(updated, 0);
        assert_eq!(created, 3);
        assert_eq!(pages.len(), 3);
    }

    #[test]
    fn test_7a6_apply_records_skip_control() {
        let mut pages = vec![(0u32, vec![0u8; PAGE_SIZE])];
        let records = make_commit_records(1, 5);
        let (applied, skipped, updated, created) = apply_records(&mut pages, &records);

        assert_eq!(applied, 0);
        assert_eq!(skipped, 5);
        assert_eq!(updated, 0);
        assert_eq!(created, 0);
    }
}
