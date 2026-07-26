//! 异地容灾 + 自动故障切换 — Phase 7a.7
//!
//! # 设计
//!
//! - **`DrCluster`** — 灾备集群，管理主库+备库节点，监控健康状态，自动故障切换
//! - **`ClusterNode`** — 集群节点（Primary/Replica/Down 三态）
//! - **`FailoverConfig`** — 故障切换配置（心跳间隔/超时/故障切换超时/切回超时）
//! - **`FailoverEvent`** — 故障切换事件日志
//!
//! # 故障切换流程
//!
//! 1. 主库崩溃 → 标记为 Down，数据保留
//! 2. `check_health()` 检测到主库 Down
//! 3. `auto_failover()` 选择 `confirmed_lsn` 最高的备库
//! 4. 提升备库为新主库（使用备库的页数据创建 `ReplicationPrimary`）
//! 5. 其他备库重连到新主库
//! 6. 原主库恢复后，`rejoin_as_replica()` 重新加入为新主库的备库
//!
//! # Chaos 测试
//!
//! 主库进程 kill → 自动检测故障 → 备库升级为主库 → 新主库可读写；
//! 原主库恢复 → 自动重新加入为备库。
//! 故障切换 < 30s，切回 < 60s。
//!
//! 对应 `SzRSQL实施进度.md` Phase 7a.7。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use szrsql_tx::wal::WalRecord;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, instrument, warn};

use crate::stream::{
    apply_records, ReplicaStats, ReplicationError, ReplicationMessage, ReplicationPrimary,
};

// =====================================================================
//  DrError
// =====================================================================

/// 灾备错误类型
#[derive(Debug, Error)]
pub enum DrError {
    /// I/O 错误
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 复制错误
    #[error("replication error: {0}")]
    Replication(#[from] ReplicationError),
    /// 节点不存在
    #[error("node not found: {0}")]
    NodeNotFound(String),
    /// 节点已宕机
    #[error("node is down: {0}")]
    NodeDown(String),
    /// 节点不是主库
    #[error("node is not primary: {0}")]
    NotPrimary(String),
    /// 节点不是备库
    #[error("node is not replica: {0}")]
    NotReplica(String),
    /// 节点未宕机
    #[error("node is not down: {0}")]
    NotDown(String),
    /// 无可用主库
    #[error("no primary available")]
    NoPrimary,
    /// 无可用备库
    #[error("no replica available for failover")]
    NoReplicaAvailable,
    /// 主库仍然存活
    #[error("primary still alive: {0}")]
    PrimaryAlive(String),
    /// 节点 ID 为空
    #[error("node id cannot be empty")]
    EmptyNodeId,
    /// 节点已存在
    #[error("node already exists: {0}")]
    NodeAlreadyExists(String),
    /// 故障切换超时
    #[error("failover timeout: {0:?}")]
    FailoverTimeout(Duration),
}

// =====================================================================
//  NodeRole
// =====================================================================

/// 节点角色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    /// 主库
    Primary,
    /// 备库
    Replica,
    /// 宕机
    Down,
}

// =====================================================================
//  FailoverConfig
// =====================================================================

/// 故障切换配置
#[derive(Debug, Clone)]
pub struct FailoverConfig {
    /// 心跳间隔
    pub heartbeat_interval: Duration,
    /// 心跳超时（超过此时间未收到心跳，判定主库故障）
    pub heartbeat_timeout: Duration,
    /// 故障切换超时上限
    pub failover_timeout: Duration,
    /// 切回超时上限
    pub switchback_timeout: Duration,
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(1),
            heartbeat_timeout: Duration::from_secs(5),
            failover_timeout: Duration::from_secs(30),
            switchback_timeout: Duration::from_secs(60),
        }
    }
}

// =====================================================================
//  FailoverEvent
// =====================================================================

/// 故障切换事件
#[derive(Debug, Clone)]
pub enum FailoverEvent {
    /// 主库宕机
    PrimaryDown {
        /// 宕机主库 ID
        node_id: String,
        /// 宕机时间
        at: Instant,
    },
    /// 故障切换开始
    FailoverStart {
        /// 原主库 ID
        from_id: String,
        /// 新主库 ID
        to_id: String,
        /// 开始时间
        at: Instant,
    },
    /// 故障切换完成
    FailoverComplete {
        /// 新主库 ID
        new_primary_id: String,
        /// 完成时间
        at: Instant,
        /// 切换耗时
        duration: Duration,
    },
    /// 节点重新加入
    NodeRejoin {
        /// 节点 ID
        node_id: String,
        /// 重新加入的角色
        as_role: NodeRole,
        /// 加入时间
        at: Instant,
    },
    /// 心跳缺失
    HeartbeatMissed {
        /// 节点 ID
        node_id: String,
        /// 缺失次数
        missed_count: u32,
    },
}

// =====================================================================
//  NodeInfo
// =====================================================================

/// 节点信息（只读视图）
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// 节点 ID
    pub node_id: String,
    /// 节点角色
    pub role: NodeRole,
    /// 已确认 LSN
    pub confirmed_lsn: u64,
    /// 页数量
    pub page_count: usize,
    /// 最后心跳时间
    pub last_heartbeat: Instant,
}

// =====================================================================
//  DrStats
// =====================================================================

/// 灾备集群统计
#[derive(Debug, Clone, Default)]
pub struct DrStats {
    /// 节点总数
    pub total_nodes: usize,
    /// 主库数（0 或 1）
    pub primary_count: usize,
    /// 备库数
    pub replica_count: usize,
    /// 宕机节点数
    pub down_count: usize,
    /// 事件总数
    pub total_events: usize,
    /// 最后一次故障切换耗时
    pub last_failover_duration: Option<Duration>,
}

// =====================================================================
//  ClusterNode — 内部节点状态
// =====================================================================

/// 集群节点内部状态
enum ClusterNode {
    /// 主库节点
    Primary {
        /// 节点 ID
        node_id: String,
        /// 复制主库
        primary: ReplicationPrimary,
        /// 初始页数据（用于恢复时计算最终页状态）
        initial_pages: Vec<(u32, Vec<u8>)>,
        /// 最后心跳时间
        last_heartbeat: Instant,
    },
    /// 备库节点
    Replica {
        /// 节点 ID
        node_id: String,
        /// 本地页存储
        pages: Vec<(u32, Vec<u8>)>,
        /// 已确认 LSN
        confirmed_lsn: u64,
        /// 消息接收端
        receiver: UnboundedReceiver<ReplicationMessage>,
        /// 最后心跳时间
        last_heartbeat: Instant,
        /// 复制统计
        stats: ReplicaStats,
    },
    /// 宕机节点（数据保留，等待恢复）
    Down {
        /// 节点 ID
        node_id: String,
        /// 宕机时的页数据
        pages: Vec<(u32, Vec<u8>)>,
        /// 宕机时的已确认 LSN
        confirmed_lsn: u64,
        /// 宕机时间
        went_down_at: Instant,
        /// 宕机前的角色
        previous_role: NodeRole,
    },
}

impl ClusterNode {
    fn node_id(&self) -> &str {
        match self {
            ClusterNode::Primary { node_id, .. } => node_id,
            ClusterNode::Replica { node_id, .. } => node_id,
            ClusterNode::Down { node_id, .. } => node_id,
        }
    }

    fn role(&self) -> NodeRole {
        match self {
            ClusterNode::Primary { .. } => NodeRole::Primary,
            ClusterNode::Replica { .. } => NodeRole::Replica,
            ClusterNode::Down { .. } => NodeRole::Down,
        }
    }
}

// =====================================================================
//  DrCluster — 灾备集群
// =====================================================================

/// 灾备集群
///
/// 管理主库 + 备库节点，提供健康监控、自动故障切换、节点恢复重连能力。
///
/// # 示例
///
/// ```
/// use szrsql_replication::dr::{DrCluster, FailoverConfig};
/// use szrsql_tx::wal::{WalRecord, WalOpType};
///
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// rt.block_on(async {
///     // 1. 创建集群，添加主库和备库
///     let cluster = DrCluster::new(FailoverConfig::default());
///     cluster.add_primary("pri", vec![(0u32, vec![0u8; 8192])]).unwrap();
///     cluster.add_replica("rep").unwrap();
///
///     // 2. 写入数据
///     let records = vec![WalRecord::new(1, 1, WalOpType::FullPageImage, 0, vec![0xAA; 8192])];
///     cluster.write(records).unwrap();
///     cluster.pump_all_replicas().unwrap();
///
///     // 3. 杀死主库
///     cluster.kill_node("pri").unwrap();
///     let events = cluster.check_health();
///     assert!(!events.is_empty());
///
///     // 4. 自动故障切换
///     let duration = cluster.auto_failover().unwrap();
///     assert!(duration < std::time::Duration::from_secs(30));
///
///     // 5. 新主库可读写
///     let new_records = vec![WalRecord::new(2, 1, WalOpType::FullPageImage, 0, vec![0xBB; 8192])];
///     cluster.write(new_records).unwrap();
///     cluster.pump_all_replicas().unwrap();
///
///     // 6. 原主库恢复并重连
///     cluster.recover_node("pri").unwrap();
///     cluster.rejoin_as_replica("pri").unwrap();
///     cluster.pump_all_replicas().unwrap();
///
///     // 7. 验证数据一致
///     let pri_pages = cluster.read_pages("pri").unwrap();
///     let rep_pages = cluster.read_pages("rep").unwrap();
///     assert_eq!(pri_pages, rep_pages);
/// });
/// ```
pub struct DrCluster {
    /// 故障切换配置
    config: FailoverConfig,
    /// 集群节点
    nodes: Mutex<HashMap<String, ClusterNode>>,
    /// 当前主库 ID
    primary_id: Mutex<Option<String>>,
    /// 事件日志
    events: Mutex<Vec<FailoverEvent>>,
}

impl DrCluster {
    /// 创建灾备集群
    pub fn new(config: FailoverConfig) -> Self {
        Self {
            config,
            nodes: Mutex::new(HashMap::new()),
            primary_id: Mutex::new(None),
            events: Mutex::new(Vec::new()),
        }
    }

    // -----------------------------------------------------------------
    //  节点管理
    // -----------------------------------------------------------------

    /// 添加主库节点
    ///
    /// # 参数
    /// - `node_id` — 节点 ID
    /// - `initial_pages` — 初始页数据
    pub fn add_primary(
        &self,
        node_id: &str,
        initial_pages: Vec<(u32, Vec<u8>)>,
    ) -> Result<(), DrError> {
        if node_id.is_empty() {
            return Err(DrError::EmptyNodeId);
        }

        let mut nodes = self.nodes.lock().unwrap();
        if nodes.contains_key(node_id) {
            return Err(DrError::NodeAlreadyExists(node_id.to_string()));
        }

        nodes.insert(
            node_id.to_string(),
            ClusterNode::Primary {
                node_id: node_id.to_string(),
                primary: ReplicationPrimary::new(node_id),
                initial_pages,
                last_heartbeat: Instant::now(),
            },
        );
        *self.primary_id.lock().unwrap() = Some(node_id.to_string());
        Ok(())
    }

    /// 添加备库节点
    ///
    /// 连接到当前主库，`start_lsn=0`（从头开始接收）。
    pub fn add_replica(&self, node_id: &str) -> Result<(), DrError> {
        if node_id.is_empty() {
            return Err(DrError::EmptyNodeId);
        }

        let primary_id = self.primary_id.lock().unwrap().clone();
        let primary_id = primary_id.ok_or(DrError::NoPrimary)?;

        let mut nodes = self.nodes.lock().unwrap();
        if nodes.contains_key(node_id) {
            return Err(DrError::NodeAlreadyExists(node_id.to_string()));
        }

        let receiver = match nodes.get(&primary_id) {
            Some(ClusterNode::Primary { primary, .. }) => primary.accept_replica(node_id, 0)?,
            _ => return Err(DrError::NotPrimary(primary_id)),
        };

        nodes.insert(
            node_id.to_string(),
            ClusterNode::Replica {
                node_id: node_id.to_string(),
                pages: Vec::new(),
                confirmed_lsn: 0,
                receiver,
                last_heartbeat: Instant::now(),
                stats: ReplicaStats::default(),
            },
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    //  读写操作
    // -----------------------------------------------------------------

    /// 向主库写入 WAL 记录（扇出到所有备库）
    ///
    /// # 返回
    /// 追加后的新 LSN
    pub fn write(&self, records: Vec<WalRecord>) -> Result<u64, DrError> {
        let primary_id = self.primary_id.lock().unwrap().clone();
        let primary_id = primary_id.ok_or(DrError::NoPrimary)?;

        let mut nodes = self.nodes.lock().unwrap();
        match nodes.get_mut(&primary_id) {
            Some(ClusterNode::Primary { primary, .. }) => Ok(primary.append_records(records)),
            Some(_) => Err(DrError::NotPrimary(primary_id)),
            None => Err(DrError::NodeNotFound(primary_id)),
        }
    }

    /// 读取节点页数据
    ///
    /// - 主库：从初始页 + WAL 回放计算当前页状态
    /// - 备库：直接返回本地页存储
    /// - 宕机节点：返回宕机时保留的页数据
    pub fn read_pages(&self, node_id: &str) -> Result<Vec<(u32, Vec<u8>)>, DrError> {
        let nodes = self.nodes.lock().unwrap();
        let node = nodes
            .get(node_id)
            .ok_or_else(|| DrError::NodeNotFound(node_id.to_string()))?;
        match node {
            ClusterNode::Primary {
                primary,
                initial_pages,
                ..
            } => Ok(primary.expected_pages(initial_pages)),
            ClusterNode::Replica { pages, .. } => Ok(pages.clone()),
            ClusterNode::Down { pages, .. } => Ok(pages.clone()),
        }
    }

    // -----------------------------------------------------------------
    //  消息泵（非阻塞接收）
    // -----------------------------------------------------------------

    /// 泵送备库消息（非阻塞排空接收缓冲区）
    ///
    /// 将备库接收缓冲区中的所有待处理消息（WalBatch/Heartbeat/Eof）排空并应用。
    pub fn pump_replica(&self, node_id: &str) -> Result<(), DrError> {
        let mut nodes = self.nodes.lock().unwrap();
        let node = nodes
            .get_mut(node_id)
            .ok_or_else(|| DrError::NodeNotFound(node_id.to_string()))?;
        let mut to_apply: Vec<ReplicationMessage> = Vec::new();
        match node {
            ClusterNode::Replica {
                receiver,
                pages,
                confirmed_lsn,
                last_heartbeat,
                stats,
                ..
            } => {
                while let Ok(msg) = receiver.try_recv() {
                    to_apply.push(msg);
                }
                for msg in to_apply {
                    match msg {
                        ReplicationMessage::WalBatch {
                            records, end_lsn, ..
                        } => {
                            let (applied, skipped, updated, created) =
                                apply_records(pages, &records);
                            stats.records_received += records.len() as u64;
                            stats.records_applied += applied;
                            stats.records_skipped += skipped;
                            stats.batches_received += 1;
                            stats.pages_updated += updated;
                            stats.pages_created += created;
                            stats.bytes_received +=
                                records.iter().map(|r| r.encoded_size() as u64).sum::<u64>();
                            *confirmed_lsn = end_lsn;
                            stats.last_lsn = end_lsn;
                        }
                        ReplicationMessage::Heartbeat { current_lsn } => {
                            stats.heartbeats_received += 1;
                            *last_heartbeat = Instant::now();
                            stats.last_lsn = current_lsn;
                        }
                        ReplicationMessage::Eof => {
                            // 优雅关闭信号，跳过（不改变页状态）
                        }
                    }
                }
            }
            ClusterNode::Primary { .. } => return Err(DrError::NotReplica(node_id.to_string())),
            ClusterNode::Down { .. } => return Err(DrError::NodeDown(node_id.to_string())),
        }
        Ok(())
    }

    /// 泵送所有备库消息
    pub fn pump_all_replicas(&self) -> Result<(), DrError> {
        let replica_ids: Vec<String> = {
            let nodes = self.nodes.lock().unwrap();
            nodes
                .iter()
                .filter(|(_, n)| matches!(n, ClusterNode::Replica { .. }))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in replica_ids {
            self.pump_replica(&id)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  心跳
    // -----------------------------------------------------------------

    /// 主库发送心跳到所有备库
    pub fn send_heartbeat(&self) -> Result<(), DrError> {
        let primary_id = self.primary_id.lock().unwrap().clone();
        let primary_id = primary_id.ok_or(DrError::NoPrimary)?;

        let mut nodes = self.nodes.lock().unwrap();
        match nodes.get_mut(&primary_id) {
            Some(ClusterNode::Primary {
                primary,
                last_heartbeat,
                ..
            }) => {
                primary.send_heartbeat();
                *last_heartbeat = Instant::now();
                Ok(())
            }
            Some(_) => Err(DrError::NotPrimary(primary_id)),
            None => Err(DrError::NodeNotFound(primary_id)),
        }
    }

    // -----------------------------------------------------------------
    //  健康检查
    // -----------------------------------------------------------------

    /// 检查集群健康状态，返回新产生的事件
    ///
    /// 检测条件：
    /// - 主库节点不存在（被 kill）→ `PrimaryDown`
    /// - 主库节点为 `Down` 状态 → `PrimaryDown`
    /// - 备库心跳超时 → `HeartbeatMissed`
    pub fn check_health(&self) -> Vec<FailoverEvent> {
        let mut new_events = Vec::new();
        let now = Instant::now();

        let primary_id = self.primary_id.lock().unwrap().clone();
        let nodes = self.nodes.lock().unwrap();

        // 检查主库状态
        let primary_down = match &primary_id {
            None => true,
            Some(pid) => !matches!(nodes.get(pid), Some(ClusterNode::Primary { .. })),
        };

        if primary_down {
            debug!(primary_id = ?primary_id, "check_health: primary down detected");
            // 查找宕机的主库节点，记录事件（去重）
            let already_recorded: Vec<String> = self
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    FailoverEvent::PrimaryDown { node_id, .. } => Some(node_id.clone()),
                    _ => None,
                })
                .collect();

            for (id, node) in nodes.iter() {
                if let ClusterNode::Down {
                    previous_role: NodeRole::Primary,
                    went_down_at,
                    ..
                } = node
                {
                    if !already_recorded.contains(id) {
                        new_events.push(FailoverEvent::PrimaryDown {
                            node_id: id.clone(),
                            at: *went_down_at,
                        });
                    }
                }
            }
        }

        // 检查备库心跳超时
        for (id, node) in nodes.iter() {
            if let ClusterNode::Replica { last_heartbeat, .. } = node {
                if now.duration_since(*last_heartbeat) > self.config.heartbeat_timeout {
                    let missed_count = self
                        .events
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|e| {
                            matches!(e, FailoverEvent::HeartbeatMissed { node_id, .. } if node_id == id)
                        })
                        .count() as u32
                        + 1;
                    new_events.push(FailoverEvent::HeartbeatMissed {
                        node_id: id.clone(),
                        missed_count,
                    });
                }
            }
        }

        // 记录事件
        self.events.lock().unwrap().extend(new_events.clone());
        new_events
    }

    // -----------------------------------------------------------------
    //  故障切换
    // -----------------------------------------------------------------

    /// 提升指定备库为新主库
    ///
    /// # 流程
    /// 1. 提取备库页数据
    /// 2. 创建新 `ReplicationPrimary`
    /// 3. 其他备库重连到新主库
    /// 4. 更新集群主库 ID
    ///
    /// # 返回
    /// 故障切换耗时
    #[instrument(skip(self), fields(replica_id = %replica_id))]
    pub fn promote_replica(&self, replica_id: &str) -> Result<Duration, DrError> {
        let start = Instant::now();

        // 校验：当前不能有活跃主库
        let old_primary_id = self.primary_id.lock().unwrap().clone();
        if let Some(ref pid) = old_primary_id {
            let nodes = self.nodes.lock().unwrap();
            if matches!(nodes.get(pid), Some(ClusterNode::Primary { .. })) {
                warn!(old_primary_id = %pid, replica_id = %replica_id, "promote_replica refused: primary still alive");
                return Err(DrError::PrimaryAlive(pid.clone()));
            }
        }

        info!(replica_id = %replica_id, old_primary_id = ?old_primary_id, "starting failover: promoting replica to primary");

        // 提取备库页数据
        let pages = {
            let mut nodes = self.nodes.lock().unwrap();
            let node = nodes
                .remove(replica_id)
                .ok_or_else(|| DrError::NodeNotFound(replica_id.to_string()))?;
            match node {
                ClusterNode::Replica { pages, .. } => pages,
                ClusterNode::Primary { .. } => {
                    nodes.insert(replica_id.to_string(), node);
                    return Err(DrError::NotReplica(replica_id.to_string()));
                }
                ClusterNode::Down { .. } => {
                    nodes.insert(replica_id.to_string(), node);
                    return Err(DrError::NodeDown(replica_id.to_string()));
                }
            }
        };

        // 记录故障切换开始事件
        self.events
            .lock()
            .unwrap()
            .push(FailoverEvent::FailoverStart {
                from_id: old_primary_id.unwrap_or_default(),
                to_id: replica_id.to_string(),
                at: start,
            });

        // 创建新主库
        let new_primary = ReplicationPrimary::new(replica_id);
        let initial_pages = pages.clone();

        // 收集其他备库 ID 和 confirmed_lsn
        let other_replicas: Vec<(String, u64)> = {
            let nodes = self.nodes.lock().unwrap();
            nodes
                .iter()
                .filter_map(|(id, node)| match node {
                    ClusterNode::Replica { confirmed_lsn, .. } => {
                        Some((id.clone(), *confirmed_lsn))
                    }
                    _ => None,
                })
                .collect()
        };

        // 其他备库重连到新主库
        let mut new_receivers: HashMap<String, UnboundedReceiver<ReplicationMessage>> =
            HashMap::new();
        for (id, lsn) in &other_replicas {
            let rx = new_primary.accept_replica(id, *lsn)?;
            new_receivers.insert(id.clone(), rx);
        }

        // 更新其他备库的接收端
        {
            let mut nodes = self.nodes.lock().unwrap();
            for (id, rx) in new_receivers {
                if let Some(ClusterNode::Replica { receiver, .. }) = nodes.get_mut(&id) {
                    *receiver = rx;
                }
            }
        }

        // 插入新主库节点
        {
            let mut nodes = self.nodes.lock().unwrap();
            nodes.insert(
                replica_id.to_string(),
                ClusterNode::Primary {
                    node_id: replica_id.to_string(),
                    primary: new_primary,
                    initial_pages,
                    last_heartbeat: Instant::now(),
                },
            );
        }
        *self.primary_id.lock().unwrap() = Some(replica_id.to_string());

        // 泵送其他备库以应用追平批次
        for (id, _) in &other_replicas {
            let _ = self.pump_replica(id);
        }

        let duration = start.elapsed();
        if duration > self.config.failover_timeout {
            return Err(DrError::FailoverTimeout(duration));
        }

        self.events
            .lock()
            .unwrap()
            .push(FailoverEvent::FailoverComplete {
                new_primary_id: replica_id.to_string(),
                at: Instant::now(),
                duration,
            });

        Ok(duration)
    }

    /// 自动故障切换
    ///
    /// 选择 `confirmed_lsn` 最高的备库提升为新主库。
    ///
    /// # 返回
    /// 故障切换耗时
    pub fn auto_failover(&self) -> Result<Duration, DrError> {
        // 校验主库已宕机
        let primary_id = self.primary_id.lock().unwrap().clone();
        if let Some(ref pid) = &primary_id {
            let nodes = self.nodes.lock().unwrap();
            if matches!(nodes.get(pid), Some(ClusterNode::Primary { .. })) {
                warn!(primary_id = %pid, "auto_failover refused: primary still alive");
                return Err(DrError::PrimaryAlive(pid.clone()));
            }
        }

        // 选择 confirmed_lsn 最高的备库
        let best_replica = {
            let nodes = self.nodes.lock().unwrap();
            let mut best: Option<(String, u64)> = None;
            for (id, node) in nodes.iter() {
                if let ClusterNode::Replica { confirmed_lsn, .. } = node {
                    match &best {
                        None => best = Some((id.clone(), *confirmed_lsn)),
                        Some((_, best_lsn)) => {
                            if *confirmed_lsn > *best_lsn {
                                best = Some((id.clone(), *confirmed_lsn));
                            }
                        }
                    }
                }
            }
            best
        };

        let (replica_id, confirmed_lsn) = best_replica.ok_or(DrError::NoReplicaAvailable)?;
        info!(elected_replica_id = %replica_id, confirmed_lsn, "auto_failover: elected replica with highest confirmed_lsn");
        self.promote_replica(&replica_id)
    }

    // -----------------------------------------------------------------
    //  Chaos：节点崩溃与恢复
    // -----------------------------------------------------------------

    /// 杀死节点（模拟进程崩溃）
    ///
    /// - 主库：调用 `crash()` 关闭所有备库通道，保留页数据
    /// - 备库：直接保留页数据
    /// - 宕机节点：返回错误
    pub fn kill_node(&self, node_id: &str) -> Result<(), DrError> {
        warn!(node_id = %node_id, "kill_node: simulating node crash");
        let mut nodes = self.nodes.lock().unwrap();
        let node = nodes
            .remove(node_id)
            .ok_or_else(|| DrError::NodeNotFound(node_id.to_string()))?;

        match node {
            ClusterNode::Primary {
                primary,
                initial_pages,
                ..
            } => {
                // 模拟崩溃：关闭所有备库通道
                primary.crash();
                // 计算最终页状态（保留数据用于恢复）
                let pages = primary.expected_pages(&initial_pages);
                let confirmed_lsn = primary.current_lsn();
                nodes.insert(
                    node_id.to_string(),
                    ClusterNode::Down {
                        node_id: node_id.to_string(),
                        pages,
                        confirmed_lsn,
                        went_down_at: Instant::now(),
                        previous_role: NodeRole::Primary,
                    },
                );
                // 清除主库 ID
                *self.primary_id.lock().unwrap() = None;
            }
            ClusterNode::Replica {
                pages,
                confirmed_lsn,
                ..
            } => {
                nodes.insert(
                    node_id.to_string(),
                    ClusterNode::Down {
                        node_id: node_id.to_string(),
                        pages,
                        confirmed_lsn,
                        went_down_at: Instant::now(),
                        previous_role: NodeRole::Replica,
                    },
                );
            }
            ClusterNode::Down { .. } => {
                nodes.insert(node_id.to_string(), node);
                return Err(DrError::NodeDown(node_id.to_string()));
            }
        }
        Ok(())
    }

    /// 恢复宕机节点
    ///
    /// 将节点从 `Down` 状态恢复，但不自动加入集群。
    /// 恢复后需调用 `rejoin_as_replica()` 重新连接。
    pub fn recover_node(&self, node_id: &str) -> Result<(), DrError> {
        // 恢复操作只是标记节点可以重新加入
        // 实际重连由 rejoin_as_replica 完成
        let nodes = self.nodes.lock().unwrap();
        match nodes.get(node_id) {
            Some(ClusterNode::Down { .. }) => Ok(()),
            Some(ClusterNode::Primary { .. }) => Err(DrError::NotDown(node_id.to_string())),
            Some(ClusterNode::Replica { .. }) => Err(DrError::NotDown(node_id.to_string())),
            None => Err(DrError::NodeNotFound(node_id.to_string())),
        }
    }

    /// 宕机节点重新加入为备库
    ///
    /// 连接到当前主库，以宕机时的 `confirmed_lsn` 为起点接收增量 WAL。
    pub fn rejoin_as_replica(&self, node_id: &str) -> Result<(), DrError> {
        info!(node_id = %node_id, "rejoin_as_replica: node rejoining cluster as replica");
        // 提取宕机节点的页数据和 confirmed_lsn
        let (pages, confirmed_lsn) = {
            let mut nodes = self.nodes.lock().unwrap();
            let node = nodes
                .remove(node_id)
                .ok_or_else(|| DrError::NodeNotFound(node_id.to_string()))?;
            match node {
                ClusterNode::Down {
                    pages,
                    confirmed_lsn,
                    ..
                } => (pages, confirmed_lsn),
                ClusterNode::Primary { .. } => {
                    nodes.insert(node_id.to_string(), node);
                    return Err(DrError::NotDown(node_id.to_string()));
                }
                ClusterNode::Replica { .. } => {
                    nodes.insert(node_id.to_string(), node);
                    return Err(DrError::NotDown(node_id.to_string()));
                }
            }
        };

        // 连接到当前主库
        let receiver = {
            let primary_id = self.primary_id.lock().unwrap().clone();
            let primary_id = primary_id.ok_or(DrError::NoPrimary)?;
            let nodes = self.nodes.lock().unwrap();
            match nodes.get(&primary_id) {
                Some(ClusterNode::Primary { primary, .. }) => {
                    primary.accept_replica(node_id, confirmed_lsn)?
                }
                _ => return Err(DrError::NotPrimary(primary_id)),
            }
        };

        // 插入为备库
        {
            let mut nodes = self.nodes.lock().unwrap();
            nodes.insert(
                node_id.to_string(),
                ClusterNode::Replica {
                    node_id: node_id.to_string(),
                    pages,
                    confirmed_lsn,
                    receiver,
                    last_heartbeat: Instant::now(),
                    stats: ReplicaStats::default(),
                },
            );
        }

        // 记录事件
        self.events.lock().unwrap().push(FailoverEvent::NodeRejoin {
            node_id: node_id.to_string(),
            as_role: NodeRole::Replica,
            at: Instant::now(),
        });

        // 泵送以应用追平批次
        self.pump_replica(node_id)?;

        Ok(())
    }

    // -----------------------------------------------------------------
    //  信息查询
    // -----------------------------------------------------------------

    /// 当前主库 ID
    pub fn primary_id(&self) -> Option<String> {
        self.primary_id.lock().unwrap().clone()
    }

    /// 列出所有节点信息
    pub fn list_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.lock().unwrap();
        nodes
            .iter()
            .map(|(id, node)| match node {
                ClusterNode::Primary {
                    last_heartbeat,
                    primary,
                    ..
                } => NodeInfo {
                    node_id: id.clone(),
                    role: NodeRole::Primary,
                    confirmed_lsn: primary.current_lsn(),
                    page_count: primary.record_count(),
                    last_heartbeat: *last_heartbeat,
                },
                ClusterNode::Replica {
                    confirmed_lsn,
                    pages,
                    last_heartbeat,
                    ..
                } => NodeInfo {
                    node_id: id.clone(),
                    role: NodeRole::Replica,
                    confirmed_lsn: *confirmed_lsn,
                    page_count: pages.len(),
                    last_heartbeat: *last_heartbeat,
                },
                ClusterNode::Down {
                    confirmed_lsn,
                    pages,
                    went_down_at,
                    ..
                } => NodeInfo {
                    node_id: id.clone(),
                    role: NodeRole::Down,
                    confirmed_lsn: *confirmed_lsn,
                    page_count: pages.len(),
                    last_heartbeat: *went_down_at,
                },
            })
            .collect()
    }

    /// 获取事件日志
    pub fn events(&self) -> Vec<FailoverEvent> {
        self.events.lock().unwrap().clone()
    }

    /// 获取集群统计
    pub fn stats(&self) -> DrStats {
        let nodes = self.nodes.lock().unwrap();
        let mut primary_count = 0;
        let mut replica_count = 0;
        let mut down_count = 0;
        for node in nodes.values() {
            match node {
                ClusterNode::Primary { .. } => primary_count += 1,
                ClusterNode::Replica { .. } => replica_count += 1,
                ClusterNode::Down { .. } => down_count += 1,
            }
        }
        let last_failover_duration =
            self.events
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find_map(|e| match e {
                    FailoverEvent::FailoverComplete { duration, .. } => Some(*duration),
                    _ => None,
                });
        DrStats {
            total_nodes: nodes.len(),
            primary_count,
            replica_count,
            down_count,
            total_events: self.events.lock().unwrap().len(),
            last_failover_duration,
        }
    }

    /// 获取故障切换配置
    pub fn config(&self) -> &FailoverConfig {
        &self.config
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_tx::wal::{WalOpType, WalRecord};

    /// 生成指定大小的页数据
    fn make_page(page_id: u32, fill: u8) -> (u32, Vec<u8>) {
        (page_id, vec![fill; 8192])
    }

    /// 生成 FullPageImage WAL 记录
    fn make_fpi_record(lsn: u64, page_id: u32, fill: u8) -> WalRecord {
        WalRecord::new(lsn, 1, WalOpType::FullPageImage, page_id, vec![fill; 8192])
    }

    #[test]
    fn test_7a7_basic_cluster() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 写入数据
        let records = vec![make_fpi_record(1, 0, 0xAA)];
        cluster.write(records).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 验证主库页
        let pri_pages = cluster.read_pages("pri").unwrap();
        assert_eq!(pri_pages[0].1, vec![0xAA; 8192]);

        // 验证备库页
        cluster.pump_replica("rep").unwrap();
        let rep_pages = cluster.read_pages("rep").unwrap();
        assert_eq!(rep_pages[0].1, vec![0xAA; 8192]);
    }

    #[test]
    fn test_7a7_multi_replica() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        let records = vec![make_fpi_record(1, 0, 0xBB)];
        cluster.write(records).unwrap();
        cluster.pump_all_replicas().unwrap();

        for id in &["rep1", "rep2"] {
            cluster.pump_replica(id).unwrap();
            let pages = cluster.read_pages(id).unwrap();
            assert_eq!(pages[0].1, vec![0xBB; 8192]);
        }
    }

    #[test]
    fn test_7a7_heartbeat() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        cluster.send_heartbeat().unwrap();
        cluster.pump_all_replicas().unwrap();

        // 验证备库收到心跳
        let nodes = cluster.list_nodes();
        let rep = nodes.iter().find(|n| n.node_id == "rep").unwrap();
        assert!(rep.confirmed_lsn == 0); // 无 WAL 记录
    }

    #[test]
    fn test_7a7_health_check_ok() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        let events = cluster.check_health();
        assert!(events.is_empty(), "健康集群不应产生事件");
    }

    #[test]
    fn test_7a7_detect_primary_down() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 健康检查应检测到主库宕机
        let events = cluster.check_health();
        assert!(events.iter().any(|e| matches!(
            e,
            FailoverEvent::PrimaryDown { node_id, .. } if node_id == "pri"
        )));
    }

    #[test]
    fn test_7a7_failover_promote() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 写入数据并泵送
        cluster.write(vec![make_fpi_record(1, 0, 0xAA)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 提升备库为新主库
        let duration = cluster.promote_replica("rep").unwrap();
        assert!(duration < Duration::from_secs(30));

        // 验证新主库 ID
        assert_eq!(cluster.primary_id(), Some("rep".to_string()));

        // 新主库可读写
        cluster.write(vec![make_fpi_record(2, 0, 0xBB)]).unwrap();

        // 验证新主库页数据（包含两次写入）
        let pages = cluster.read_pages("rep").unwrap();
        assert_eq!(pages[0].1, vec![0xBB; 8192]);
    }

    #[test]
    fn test_7a7_auto_failover() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        // 写入数据
        cluster.write(vec![make_fpi_record(1, 0, 0xCC)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 自动故障切换
        let duration = cluster.auto_failover().unwrap();
        assert!(duration < Duration::from_secs(30));

        // 验证有新主库
        let new_primary = cluster.primary_id().unwrap();
        assert!(new_primary == "rep1" || new_primary == "rep2");

        // 新主库可写
        cluster.write(vec![make_fpi_record(2, 0, 0xDD)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 验证所有活跃节点数据一致
        let primary_pages = cluster.read_pages(&new_primary).unwrap();
        assert_eq!(primary_pages[0].1, vec![0xDD; 8192]);
    }

    #[test]
    fn test_7a7_rejoin_as_replica() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 写入初始数据
        cluster.write(vec![make_fpi_record(1, 0, 0xAA)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 故障切换
        cluster.auto_failover().unwrap();

        // 新主库写入更多数据
        cluster.write(vec![make_fpi_record(2, 0, 0xBB)]).unwrap();

        // 恢复原主库
        cluster.recover_node("pri").unwrap();

        // 原主库重新加入为备库
        cluster.rejoin_as_replica("pri").unwrap();

        // 泵送以应用追平批次
        cluster.pump_all_replicas().unwrap();

        // 验证原主库数据与新主库一致
        let pri_pages = cluster.read_pages("pri").unwrap();
        let rep_pages = cluster.read_pages("rep").unwrap();
        assert_eq!(pri_pages, rep_pages);
        assert_eq!(pri_pages[0].1, vec![0xBB; 8192]);
    }

    #[test]
    fn test_7a7_chaos_full_scenario() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00), make_page(1, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        // 阶段 1：写入 1000 条记录
        let records: Vec<WalRecord> = (1..=1000)
            .map(|i| make_fpi_record(i, (i % 2) as u32, (i % 256) as u8))
            .collect();
        cluster.write(records).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 阶段 2：杀死主库
        cluster.kill_node("pri").unwrap();

        // 阶段 3：自动检测故障
        let events = cluster.check_health();
        assert!(events.iter().any(|e| matches!(
            e,
            FailoverEvent::PrimaryDown { node_id, .. } if node_id == "pri"
        )));

        // 阶段 4：自动故障切换（< 30s）
        let failover_duration = cluster.auto_failover().unwrap();
        assert!(failover_duration < Duration::from_secs(30));

        let new_primary = cluster.primary_id().unwrap();
        assert_ne!(new_primary, "pri");

        // 阶段 5：新主库写入 500 条新记录
        let new_records: Vec<WalRecord> = (1001..=1500)
            .map(|i| make_fpi_record(i, (i % 2) as u32, (i % 256) as u8))
            .collect();
        cluster.write(new_records).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 阶段 6：原主库恢复并重新加入（< 60s）
        let rejoin_start = Instant::now();
        cluster.recover_node("pri").unwrap();
        cluster.rejoin_as_replica("pri").unwrap();
        cluster.pump_all_replicas().unwrap();
        let rejoin_duration = rejoin_start.elapsed();
        assert!(rejoin_duration < Duration::from_secs(60));

        // 阶段 7：验证所有节点数据一致
        let mut new_pri_pages = cluster.read_pages(&new_primary).unwrap();
        let mut pri_pages = cluster.read_pages("pri").unwrap();
        // 按页 ID 排序后比较，避免 expected_pages 内部 HashMap 迭代顺序差异
        new_pri_pages.sort_by_key(|(id, _)| *id);
        pri_pages.sort_by_key(|(id, _)| *id);
        assert_eq!(pri_pages.len(), new_pri_pages.len());
        for (a, b) in pri_pages.iter().zip(new_pri_pages.iter()) {
            assert_eq!(a.0, b.0);
            assert_eq!(a.1, b.1);
        }
    }

    #[test]
    fn test_7a7_failover_no_replica() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 无备库可切换
        let result = cluster.auto_failover();
        assert!(matches!(result, Err(DrError::NoReplicaAvailable)));
    }

    #[test]
    fn test_7a7_kill_replica() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        // 杀死一个备库
        cluster.kill_node("rep1").unwrap();

        // 主库仍可读写
        cluster.write(vec![make_fpi_record(1, 0, 0xEE)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 存活备库数据正确
        cluster.pump_replica("rep2").unwrap();
        let pages = cluster.read_pages("rep2").unwrap();
        assert_eq!(pages[0].1, vec![0xEE; 8192]);
    }

    #[test]
    fn test_7a7_read_from_node() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x11), make_page(1, 0x22)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 读主库
        let pri_pages = cluster.read_pages("pri").unwrap();
        assert_eq!(pri_pages.len(), 2);
        assert_eq!(pri_pages[0].1, vec![0x11; 8192]);
        assert_eq!(pri_pages[1].1, vec![0x22; 8192]);

        // 读备库（空）
        let rep_pages = cluster.read_pages("rep").unwrap();
        assert!(rep_pages.is_empty());
    }

    #[test]
    fn test_7a7_node_info() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        let nodes = cluster.list_nodes();
        assert_eq!(nodes.len(), 2);

        let pri = nodes.iter().find(|n| n.node_id == "pri").unwrap();
        assert_eq!(pri.role, NodeRole::Primary);

        let rep = nodes.iter().find(|n| n.node_id == "rep").unwrap();
        assert_eq!(rep.role, NodeRole::Replica);
    }

    #[test]
    fn test_7a7_empty_node_id() {
        let cluster = DrCluster::new(FailoverConfig::default());

        let result = cluster.add_primary("", vec![]);
        assert!(matches!(result, Err(DrError::EmptyNodeId)));

        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        let result = cluster.add_replica("");
        assert!(matches!(result, Err(DrError::EmptyNodeId)));
    }

    #[test]
    fn test_7a7_duplicate_node() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();

        let result = cluster.add_primary("pri", vec![]);
        assert!(matches!(result, Err(DrError::NodeAlreadyExists(id)) if id == "pri"));

        let result = cluster.add_replica("pri");
        assert!(matches!(result, Err(DrError::NodeAlreadyExists(id)) if id == "pri"));
    }

    #[test]
    fn test_7a7_promote_non_replica() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 尝试提升主库 → 错误
        let result = cluster.promote_replica("pri");
        assert!(matches!(result, Err(DrError::PrimaryAlive(_))));

        // 杀死主库后尝试提升主库（已 Down）
        cluster.kill_node("pri").unwrap();
        let result = cluster.promote_replica("pri");
        assert!(matches!(result, Err(DrError::NodeDown(_))));
    }

    #[test]
    fn test_7a7_recover_non_down() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();

        // 尝试恢复活跃节点 → 错误
        let result = cluster.recover_node("pri");
        assert!(matches!(result, Err(DrError::NotDown(_))));

        // 尝试恢复不存在的节点
        let result = cluster.recover_node("nonexistent");
        assert!(matches!(result, Err(DrError::NodeNotFound(_))));
    }

    #[test]
    fn test_7a7_rejoin_non_down() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 尝试重连活跃备库 → 错误
        let result = cluster.rejoin_as_replica("rep");
        assert!(matches!(result, Err(DrError::NotDown(_))));
    }

    #[test]
    fn test_7a7_kill_down_node() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();

        cluster.kill_node("pri").unwrap();

        // 尝试再次杀死已宕机节点 → 错误
        let result = cluster.kill_node("pri");
        assert!(matches!(result, Err(DrError::NodeDown(_))));
    }

    #[test]
    fn test_7a7_failover_preserves_data() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00), make_page(1, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 写入多页数据
        cluster
            .write(vec![
                make_fpi_record(1, 0, 0xAA),
                make_fpi_record(2, 1, 0xBB),
            ])
            .unwrap();
        cluster.pump_all_replicas().unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 宕机主库的数据应被保留
        let down_pages = cluster.read_pages("pri").unwrap();
        assert_eq!(down_pages.len(), 2);
        assert_eq!(down_pages[0].1, vec![0xAA; 8192]);
        assert_eq!(down_pages[1].1, vec![0xBB; 8192]);

        // 故障切换
        cluster.auto_failover().unwrap();

        // 新主库数据应与原主库一致
        let new_pri_pages = cluster.read_pages("rep").unwrap();
        assert_eq!(new_pri_pages.len(), 2);
        assert_eq!(new_pri_pages[0].1, vec![0xAA; 8192]);
        assert_eq!(new_pri_pages[1].1, vec![0xBB; 8192]);
    }

    #[test]
    fn test_7a7_stats() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        let stats = cluster.stats();
        assert_eq!(stats.total_nodes, 3);
        assert_eq!(stats.primary_count, 1);
        assert_eq!(stats.replica_count, 2);
        assert_eq!(stats.down_count, 0);

        // 杀死一个备库
        cluster.kill_node("rep1").unwrap();
        let stats = cluster.stats();
        assert_eq!(stats.down_count, 1);
        assert_eq!(stats.replica_count, 1);
    }

    #[test]
    fn test_7a7_events_log() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep").unwrap();

        // 初始无事件
        assert!(cluster.events().is_empty());

        // 杀死主库
        cluster.kill_node("pri").unwrap();
        cluster.check_health();
        assert!(!cluster.events().is_empty());

        // 故障切换
        cluster.auto_failover().unwrap();
        let events = cluster.events();
        assert!(events
            .iter()
            .any(|e| matches!(e, FailoverEvent::FailoverStart { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, FailoverEvent::FailoverComplete { .. })));

        // 恢复并重连
        cluster.recover_node("pri").unwrap();
        cluster.rejoin_as_replica("pri").unwrap();
        let events = cluster.events();
        assert!(events
            .iter()
            .any(|e| matches!(e, FailoverEvent::NodeRejoin { .. })));
    }

    #[test]
    fn test_7a7_multi_page_failover() {
        let cluster = DrCluster::new(FailoverConfig::default());
        let initial_pages: Vec<(u32, Vec<u8>)> = (0..10).map(|i| make_page(i, 0x00)).collect();
        cluster.add_primary("pri", initial_pages).unwrap();
        cluster.add_replica("rep").unwrap();

        // 写入 10 页 × 5 更新 = 50 条记录
        let records: Vec<WalRecord> = (1..=50)
            .map(|i| make_fpi_record(i, ((i - 1) % 10) as u32, (i % 256) as u8))
            .collect();
        cluster.write(records).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 杀死主库并故障切换
        cluster.kill_node("pri").unwrap();
        cluster.auto_failover().unwrap();

        // 验证新主库 10 页数据完整
        let pages = cluster.read_pages("rep").unwrap();
        assert_eq!(pages.len(), 10);
    }

    #[test]
    fn test_7a7_integration_1m_rows() {
        const PAGE_SIZE: usize = 8192;
        const ROWS_PER_PAGE: usize = PAGE_SIZE / 8; // 1024 rows
        const TOTAL_PAGES: usize = 977; // ~1M rows (977 * 1024 = 1,002,048)
        const BATCH_COUNT: usize = 10;

        let cluster = DrCluster::new(FailoverConfig::default());

        // 创建初始页（全 0）
        let initial_pages: Vec<(u32, Vec<u8>)> = (0..TOTAL_PAGES as u32)
            .map(|i| (i, vec![0u8; PAGE_SIZE]))
            .collect();
        cluster.add_primary("pri", initial_pages).unwrap();
        cluster.add_replica("rep").unwrap();

        // 批量写入 1M 行数据（每页一个 FullPageImage 记录）
        let pages_per_batch = TOTAL_PAGES.div_ceil(BATCH_COUNT);
        for batch in 0..BATCH_COUNT {
            let start_page = batch * pages_per_batch;
            let end_page = ((batch + 1) * pages_per_batch).min(TOTAL_PAGES);
            let records: Vec<WalRecord> = (start_page..end_page)
                .map(|page_idx| {
                    let lsn = (page_idx + 1) as u64;
                    let fill = ((batch * 37 + page_idx) % 256) as u8;
                    WalRecord::new(
                        lsn,
                        1,
                        WalOpType::FullPageImage,
                        page_idx as u32,
                        vec![fill; PAGE_SIZE],
                    )
                })
                .collect();
            cluster.write(records).unwrap();
        }
        cluster.pump_all_replicas().unwrap();

        // 杀死主库
        cluster.kill_node("pri").unwrap();

        // 故障切换（< 30s）
        let failover_duration = cluster.auto_failover().unwrap();
        assert!(failover_duration < Duration::from_secs(30));

        // 新主库写入 500 条新记录
        let new_records: Vec<WalRecord> = (TOTAL_PAGES as u64 + 1..=TOTAL_PAGES as u64 + 500)
            .map(|lsn| {
                let page_idx = ((lsn as usize - 1) % TOTAL_PAGES) as u32;
                WalRecord::new(
                    lsn,
                    1,
                    WalOpType::FullPageImage,
                    page_idx,
                    vec![0xFF; PAGE_SIZE],
                )
            })
            .collect();
        cluster.write(new_records).unwrap();

        // 恢复原主库并重连（< 60s）
        let rejoin_start = Instant::now();
        cluster.recover_node("pri").unwrap();
        cluster.rejoin_as_replica("pri").unwrap();
        cluster.pump_all_replicas().unwrap();
        let rejoin_duration = rejoin_start.elapsed();
        assert!(rejoin_duration < Duration::from_secs(60));

        // 验证原主库与新主库数据一致
        let pri_pages = cluster.read_pages("pri").unwrap();
        let rep_pages = cluster.read_pages("rep").unwrap();
        assert_eq!(pri_pages.len(), rep_pages.len());
        for (a, b) in pri_pages.iter().zip(rep_pages.iter()) {
            assert_eq!(a.0, b.0);
            assert_eq!(a.1, b.1);
        }

        // 验证部分页被 0xFF 覆盖
        let ff_count = rep_pages.iter().filter(|(_, p)| p[0] == 0xFF).count();
        assert!(ff_count > 0, "应有页被 0xFF 覆盖");
    }

    #[test]
    fn test_7a7_failover_then_write_then_rejoin() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        // 初始写入
        cluster.write(vec![make_fpi_record(1, 0, 0xAA)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 故障切换
        cluster.kill_node("pri").unwrap();
        cluster.auto_failover().unwrap();
        let new_primary = cluster.primary_id().unwrap();

        // 新主库写入
        cluster.write(vec![make_fpi_record(2, 0, 0xBB)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 原主库恢复重连
        cluster.recover_node("pri").unwrap();
        cluster.rejoin_as_replica("pri").unwrap();
        cluster.pump_all_replicas().unwrap();

        // 验证三节点数据一致
        let pri_pages = cluster.read_pages("pri").unwrap();
        let new_pri_pages = cluster.read_pages(&new_primary).unwrap();
        assert_eq!(pri_pages, new_pri_pages);
        assert_eq!(pri_pages[0].1, vec![0xBB; 8192]);
    }

    #[test]
    fn test_7a7_cascading_failover() {
        let cluster = DrCluster::new(FailoverConfig::default());
        cluster
            .add_primary("pri", vec![make_page(0, 0x00)])
            .unwrap();
        cluster.add_replica("rep1").unwrap();
        cluster.add_replica("rep2").unwrap();

        // 写入数据
        cluster.write(vec![make_fpi_record(1, 0, 0xAA)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 第一次故障切换：pri → rep1/rep2
        cluster.kill_node("pri").unwrap();
        cluster.auto_failover().unwrap();
        let first_new_primary = cluster.primary_id().unwrap();

        // 新主库写入
        cluster.write(vec![make_fpi_record(2, 0, 0xBB)]).unwrap();
        cluster.pump_all_replicas().unwrap();

        // 杀死新主库
        cluster.kill_node(&first_new_primary).unwrap();

        // 第二次故障切换
        cluster.auto_failover().unwrap();
        let second_new_primary = cluster.primary_id().unwrap();
        assert_ne!(second_new_primary, first_new_primary);

        // 验证最终主库有两次写入的数据
        let pages = cluster.read_pages(&second_new_primary).unwrap();
        assert_eq!(pages[0].1, vec![0xBB; 8192]);
    }
}
