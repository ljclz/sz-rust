//! P0-DIST-1/2/3：分布式运行时集成层
//!
//! 将 Raft 共识、TSO 时间戳服务、Multi-Raft 分片整合为统一的 `DistRuntime`，
//! 作为 Arc 共享资源注入到 szrsql 主运行时（main.rs → session → executor）。
//!
//! # 设计目标
//!
//! 1. **持久化生命周期**：`DistRuntime` 通过 `Arc<RwLock<DistRuntime>>` 共享，
//!    不会被 main 函数作用域回收。
//! 2. **真实 Raft 写入路径**：`put`/`delete` 通过 `propose → append → apply`
//!    完整走 Raft 日志复制流程，而非直接写入内存 Map。
//! 3. **TSO 集成**：`begin_transaction` 从 TSO 获取单调递增的 start_ts，
//!    用于 Percolator 两阶段提交。
//! 4. **分片路由**：`route(key)` 返回键所属分片 ID，支持后续多分片扩展。
//! 5. **单节点模式**：当前迭代为单节点（self-elect Leader），证明 Raft 模块
//!    真实接入运行时；后续迭代扩展为多节点集群。
//!
//! # 与现有组件的关系
//!
//! - **RaftNode**：复用 `szrsql_dist::raft::RaftNode`，单节点自选举为 Leader
//! - **KvStateMachine**：复用 `szrsql_dist::shard::KvStateMachine`，应用已提交日志
//! - **ShardRouter**：复用 `szrsql_dist::shard::ShardRouter`，单分片覆盖全键空间
//! - **TimestampOracle**：复用 `szrsql_dist::txn::TimestampOracle`，全局时间戳
//! - **MultiRaftNode**：复用 `szrsql_dist::shard::MultiRaftNode`，托管分片 Raft 组
//!
//! # 迭代计划
//!
//! - **迭代 1（当前）**：单节点 DistRuntime，Raft propose → apply 真实路径
//! - **迭代 2（后续）**：多节点集群，跨节点日志复制 + 故障恢复
//! - **迭代 3（后续）**：Percolator 跨分片 2PC，TSO 与 MVCC 协同

use crate::raft::{Index, NodeId, RaftError, RaftNetwork, RaftState, RpcMessage};
use crate::shard::{
    KeyRange, KvStateMachine, MultiRaftNode, Shard, ShardCommand, ShardId, ShardRouter,
};
use crate::txn::TimestampOracle;
use std::collections::HashMap;
use std::sync::Arc;
// P0-6：使用 parking_lot::RwLock 替代 std::sync::RwLock，消除中毒 panic 风险
use parking_lot::RwLock;

// =====================================================================
//  DistRuntimeError — 运行时错误
// =====================================================================

/// 分布式运行时错误
#[derive(Debug, thiserror::Error)]
pub enum DistRuntimeError {
    /// Raft 错误（propose 失败、非 Leader 等）
    #[error("raft error: {0}")]
    Raft(#[from] RaftError),

    /// 键路由失败（无分片覆盖）
    #[error("route error: {0}")]
    Route(String),

    /// 分片不存在
    #[error("shard not found: {0}")]
    ShardNotFound(ShardId),

    /// 非 Leader 节点（单节点模式下不应发生）
    #[error("not leader: node {0}")]
    NotLeader(NodeId),

    /// 序列化错误
    #[error("serialization error: {0}")]
    Serialization(String),
}

// =====================================================================
//  DistRuntime — 分布式运行时
// =====================================================================

/// 分布式运行时：整合 Raft/TSO/Multi-Raft 的单节点运行时。
///
/// **线程安全**：内部状态通过 `Mutex` 保护，`DistRuntimeHandle`（Arc 包装）
/// 可安全跨线程共享。所有写操作（put/delete/propose）都会获取锁。
///
/// **生命周期**：由 main.rs 创建为 `Arc<RwLock<DistRuntime>>`，注入到
/// session/executor，随服务器生命周期存活。
///
/// **当前限制**（迭代 1）：
/// - 单节点（无跨节点日志复制）
/// - 单分片（无 Range-based 分裂）
/// - TSO 仅用于时间戳分配（无跨分片 Percolator 2PC）
pub struct DistRuntime {
    /// 物理节点 ID（单节点模式固定为 1）
    node_id: NodeId,
    /// Multi-Raft 物理节点（托管所有分片的 Raft 组）
    multi_raft: MultiRaftNode,
    /// TSO 时间戳服务（Percolator 协调器）
    tso: TimestampOracle,
    /// 分片 ID → KV 状态机（应用已提交日志后的数据）
    ///
    /// 注：MultiRaftNode 内部也维护状态机，此处冗余引用用于快速读取。
    /// 写入路径：propose → Raft append → apply → 更新 MultiRaftNode 内部状态机
    /// 读取路径：直接从此处读取，避免每次都走 Raft
    state_machines: HashMap<ShardId, KvStateMachine>,
    /// 是否已初始化（Raft 已自选举为 Leader）
    initialized: bool,
}

impl DistRuntime {
    /// 创建单节点分布式运行时
    ///
    /// # 流程
    /// 1. 创建单分片（ShardId=1，无界 KeyRange，peers=[1]）
    /// 2. 创建 MultiRaftNode 并加入分片
    /// 3. 初始化 Raft 组并自选举为 Leader
    /// 4. 初始化 TSO
    ///
    /// # 参数
    /// - `node_id`：物理节点 ID（单节点模式建议为 1）
    pub fn new_single_node(node_id: NodeId) -> Result<Self, DistRuntimeError> {
        let mut router = ShardRouter::new();
        let full_range = KeyRange::unbounded();
        let shard = Shard::new(1, full_range, vec![node_id]);
        router.add_shard(shard.clone());

        let mut multi_raft = MultiRaftNode::new(node_id, router);
        // join_shard 创建 Raft 组（单节点配置）
        multi_raft.join_shard(&shard, 42);

        // 自选举为 Leader：单节点模式下直接 become_candidate → become_leader
        // 注：MultiRaftNode 没有暴露 RaftNode 的可变引用，需要通过内部接口
        // 这里通过 propose 前的状态检查确保是 Leader
        let mut state_machines = HashMap::new();
        state_machines.insert(shard.id, KvStateMachine::new());

        let runtime = Self {
            node_id,
            multi_raft,
            tso: TimestampOracle::new(),
            state_machines,
            initialized: false,
        };

        Ok(runtime)
    }

    /// P0-DIST 迭代 2：创建多节点集群中的一个节点
    ///
    /// 与 `new_single_node` 不同，此构造器：
    /// 1. 分片 peers 包含所有节点（非仅自己）
    /// 2. 不调用 `promote_to_leader`（由集群 tick 触发自然选举）
    /// 3. 每个 RaftNode 的 `Config.peers` 包含其他所有节点
    ///
    /// # 参数
    /// - `node_id`：本节点 ID
    /// - `all_nodes`：集群所有节点 ID（含本节点）
    /// - `seed`：确定性随机种子（所有节点必须相同，确保选举超时可复现）
    pub fn new_cluster_node(
        node_id: NodeId,
        all_nodes: &[NodeId],
        seed: u64,
    ) -> Result<Self, DistRuntimeError> {
        let mut router = ShardRouter::new();
        let full_range = KeyRange::unbounded();
        // 分片 peers = 所有节点
        let shard = Shard::new(1, full_range, all_nodes.to_vec());
        router.add_shard(shard.clone());

        let mut multi_raft = MultiRaftNode::new(node_id, router);
        // join_shard 会根据 shard.peers 过滤掉自己，设置 Config.peers = 其他节点
        multi_raft.join_shard(&shard, seed);

        let mut state_machines = HashMap::new();
        state_machines.insert(shard.id, KvStateMachine::new());

        Ok(Self {
            node_id,
            multi_raft,
            tso: TimestampOracle::new(),
            state_machines,
            initialized: false,
        })
    }

    /// 初始化 Raft 组：自选举为 Leader
    ///
    /// 必须在 `put`/`delete`/`propose` 之前调用。
    /// 单节点模式下，通过 `MultiRaftNode::promote_to_leader` 将所有分片的
    /// Raft 组从 Follower → Candidate → Leader。
    pub fn init(&mut self) -> Result<(), DistRuntimeError> {
        if self.initialized {
            return Ok(());
        }
        // 将所有分片的 Raft 组自选举为 Leader（单节点模式）
        let shard_ids = self.multi_raft.shard_ids();
        for sid in shard_ids {
            self.multi_raft.promote_to_leader(sid)?;
            tracing::debug!(
                node_id = self.node_id,
                shard_id = sid,
                "Raft group promoted to Leader (single-node)"
            );
        }
        self.initialized = true;
        tracing::info!(
            node_id = self.node_id,
            shard_count = self.multi_raft.shard_ids().len(),
            "DistRuntime initialized (single-node mode, all shards self-elected as Leader)"
        );
        Ok(())
    }

    /// P0-DIST 迭代 3：创建多分片单节点运行时
    ///
    /// 与 `new_single_node`（单分片无界）不同，此构造器创建多个分片，
    /// 每个分片覆盖不同的 KeyRange，由独立的 Raft 组管理。
    ///
    /// # 参数
    /// - `node_id`：物理节点 ID
    /// - `shards`：分片定义列表（每个分片需指定 id、range、peers）
    /// - `seed`：确定性随机种子
    ///
    /// # 示例
    /// ```ignore
    /// let shards = vec![
    ///     Shard::new(1, KeyRange::new(b"a".to_vec(), b"m".to_vec()), vec![1]),
    ///     Shard::new(2, KeyRange::from(b"m".to_vec()), vec![1]),
    /// ];
    /// let mut rt = DistRuntime::new_multi_shard_single_node(1, shards, 42)?;
    /// rt.init()?;
    /// // key "apple" → shard 1, key "zebra" → shard 2
    /// ```
    pub fn new_multi_shard_single_node(
        node_id: NodeId,
        shards: Vec<Shard>,
        seed: u64,
    ) -> Result<Self, DistRuntimeError> {
        if shards.is_empty() {
            return Err(DistRuntimeError::Route(
                "multi-shard runtime requires at least 1 shard".into(),
            ));
        }

        let mut router = ShardRouter::new();
        let mut multi_raft = MultiRaftNode::new(node_id, router.clone());
        let mut state_machines = HashMap::new();

        for shard in &shards {
            router.add_shard(shard.clone());
            multi_raft.join_shard(shard, seed + shard.id);
            state_machines.insert(shard.id, KvStateMachine::new());
        }

        // 同步路由器到 MultiRaftNode
        multi_raft.router = router;

        Ok(Self {
            node_id,
            multi_raft,
            tso: TimestampOracle::new(),
            state_machines,
            initialized: false,
        })
    }

    /// P0-DIST 迭代 3：动态添加分片
    ///
    /// 向运行时添加新分片（例如分片分裂后添加新分片）。
    /// 新分片的 Raft 组会立即创建并（若已 init）自选举为 Leader。
    ///
    /// # 参数
    /// - `shard`：分片定义
    /// - `seed`：Raft 随机种子
    pub fn add_shard(&mut self, shard: Shard, seed: u64) -> Result<(), DistRuntimeError> {
        self.multi_raft.router.add_shard(shard.clone());
        self.multi_raft.join_shard(&shard, seed);
        self.state_machines.insert(shard.id, KvStateMachine::new());

        // 若已初始化，立即将新分片提升为 Leader（单节点模式）
        if self.initialized {
            self.multi_raft.promote_to_leader(shard.id)?;
            tracing::debug!(
                node_id = self.node_id,
                shard_id = shard.id,
                "New shard added and promoted to Leader"
            );
        }
        Ok(())
    }

    /// P0-DIST 迭代 3：获取所有分片信息
    pub fn shards(&self) -> &HashMap<ShardId, Shard> {
        self.multi_raft.router.shards()
    }

    /// P0-DIST 迭代 3：获取分片数量
    pub fn shard_count(&self) -> usize {
        self.multi_raft.shard_ids().len()
    }

    /// P0-DIST 迭代 3：获取指定分片的键数量
    pub fn shard_kv_len(&self, shard_id: ShardId) -> Result<usize, DistRuntimeError> {
        let sm = self
            .multi_raft
            .state_machine(shard_id)
            .ok_or(DistRuntimeError::ShardNotFound(shard_id))?;
        Ok(sm.len())
    }

    /// 获取节点 ID
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// 获取 TSO 当前时间戳（不递增）
    pub fn current_timestamp(&self) -> u64 {
        self.tso.current()
    }

    /// 从 TSO 获取新的事务开始时间戳（单调递增）
    pub fn begin_transaction(&mut self) -> u64 {
        self.tso.get_ts()
    }

    /// 路由键到分片 ID
    ///
    /// # Errors
    /// 若无分片覆盖该键，返回 `RouteError`
    pub fn route(&self, key: &[u8]) -> Result<ShardId, DistRuntimeError> {
        self.multi_raft
            .router
            .route(key)
            .map_err(|e| DistRuntimeError::Route(format!("{:?}", e)))
    }

    /// 向指定分片提议写入命令（Put/Delete）
    ///
    /// # 流程
    /// 1. 路由键到分片
    /// 2. 检查本节点是否为该分片的 Leader
    /// 3. 调用 `MultiRaftNode::propose` 提交命令到 Raft 日志
    /// 4. 命令经 Raft 复制后 apply 到状态机
    ///
    /// # Errors
    /// - 非 Leader
    /// - Raft propose 失败
    pub fn propose(
        &mut self,
        shard_id: ShardId,
        command: ShardCommand,
    ) -> Result<Index, DistRuntimeError> {
        // 检查是否为 Leader
        if self.multi_raft.shard_leader(shard_id).is_none() {
            return Err(DistRuntimeError::NotLeader(self.node_id));
        }
        let idx = self.multi_raft.propose(shard_id, command)?;
        Ok(idx)
    }

    /// 推进 Raft 时钟并应用已提交日志到状态机
    ///
    /// 单节点模式下，propose 后立即调用 tick 可将命令 apply 到状态机。
    /// 多节点模式下，需等待多数派复制后才能 apply。
    ///
    /// # 参数
    /// - `elapsed_ms`：逻辑时间增量（毫秒）
    pub fn tick(&mut self, elapsed_ms: u64) {
        let _messages = self.multi_raft.tick(elapsed_ms);
        // 注：单节点模式下，propose 的命令在 tick 后会立即 apply
        // 多节点模式下，需将 messages 投递到其他节点
    }

    /// 向 KV 存储写入键值（经过 Raft propose → apply）
    ///
    /// # 流程
    /// 1. 路由键到分片
    /// 2. propose Put 命令到 Raft
    /// 3. tick 推进 apply
    /// 4. 更新本地状态机缓存
    ///
    /// # Errors
    /// - 路由失败
    /// - Raft propose 失败
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), DistRuntimeError> {
        let shard_id = self.route(&key)?;
        let command = ShardCommand::Put {
            key: key.clone(),
            value,
        };
        self.propose(shard_id, command)?;
        // 推进 apply：单节点模式下立即生效
        self.tick(10);
        // 同步到本地状态机缓存
        self.sync_state_machine(shard_id);
        Ok(())
    }

    /// 从 KV 存储删除键（经过 Raft propose → apply）
    ///
    /// # Errors
    /// - 路由失败
    /// - Raft propose 失败
    pub fn delete(&mut self, key: Vec<u8>) -> Result<(), DistRuntimeError> {
        let shard_id = self.route(&key)?;
        let command = ShardCommand::Delete { key: key.clone() };
        self.propose(shard_id, command)?;
        self.tick(10);
        self.sync_state_machine(shard_id);
        Ok(())
    }

    /// 从 KV 存储读取键值
    ///
    /// **读取语义**：从本地状态机读取，保证已 apply 的数据可见。
    /// 单节点模式下等价于强一致性读。
    ///
    /// # 返回
    /// - `Ok(Some(value))`：键存在
    /// - `Ok(None)`：键不存在
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, DistRuntimeError> {
        let shard_id = self.route(key)?;
        let sm = self
            .multi_raft
            .state_machine(shard_id)
            .ok_or(DistRuntimeError::ShardNotFound(shard_id))?;
        Ok(sm.get(key).map(|v| v.to_vec()))
    }

    /// 范围扫描 [start, end)
    ///
    /// # 返回
    /// 有序的 (key, value) 列表
    pub fn scan(&self, range: &KeyRange) -> Result<Vec<(Vec<u8>, Vec<u8>)>, DistRuntimeError> {
        let shard_ids = self.multi_raft.router.route_range(range);
        let mut results = Vec::new();
        for sid in shard_ids {
            if let Some(sm) = self.multi_raft.state_machine(sid) {
                results.extend(sm.scan(range));
            }
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    // ==================================================================
    //  P0-DIST 阶段 2：分片级操作（支持 Percolator 原始键路由）
    // ==================================================================

    /// 路由原始键到分片 ID（不经过前缀编码）
    ///
    /// Percolator 使用 0x01/0x02/0x03 前缀编码 data/lock/write 键，
    /// 这些前缀会改变键的排序，导致基于编码键的路由将同一逻辑键的
    /// data/lock/write 记录分散到不同分片。
    ///
    /// 此方法允许调用方先用原始键路由获取 shard_id，
    /// 然后用 `put_shard`/`get_shard`/`scan_shard`/`delete_shard`
    /// 在指定分片上操作编码后的键。
    pub fn route_raw_key(&self, key: &[u8]) -> Result<ShardId, DistRuntimeError> {
        self.multi_raft
            .router
            .route(key)
            .map_err(|e| DistRuntimeError::Route(format!("{:?}", e)))
    }

    /// 向指定分片写入键值（不经路由，由调用方保证 shard_id 正确）
    ///
    /// 用于 Percolator 事务：调用方先用 `route_raw_key` 获取 shard_id，
    /// 然后用此方法在正确的分片上写入编码后的 data/lock/write 键。
    pub fn put_shard(
        &mut self,
        shard_id: ShardId,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<(), DistRuntimeError> {
        let command = ShardCommand::Put {
            key: key.clone(),
            value,
        };
        self.propose(shard_id, command)?;
        self.tick(10);
        self.sync_state_machine(shard_id);
        Ok(())
    }

    /// 从指定分片删除键（不经路由）
    pub fn delete_shard(
        &mut self,
        shard_id: ShardId,
        key: Vec<u8>,
    ) -> Result<(), DistRuntimeError> {
        let command = ShardCommand::Delete { key: key.clone() };
        self.propose(shard_id, command)?;
        self.tick(10);
        self.sync_state_machine(shard_id);
        Ok(())
    }

    /// 从指定分片读取键值（不经路由）
    pub fn get_shard(
        &self,
        shard_id: ShardId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, DistRuntimeError> {
        let sm = self
            .multi_raft
            .state_machine(shard_id)
            .ok_or(DistRuntimeError::ShardNotFound(shard_id))?;
        Ok(sm.get(key).map(|v| v.to_vec()))
    }

    /// 从指定分片范围扫描（不经路由）
    ///
    /// 返回该分片内 `range` 范围内的所有 (key, value) 对（已排序）。
    pub fn scan_shard(
        &self,
        shard_id: ShardId,
        range: &KeyRange,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, DistRuntimeError> {
        let sm = self
            .multi_raft
            .state_machine(shard_id)
            .ok_or(DistRuntimeError::ShardNotFound(shard_id))?;
        Ok(sm.scan(range))
    }

    /// 获取分片 ID 列表
    pub fn shard_ids(&self) -> Vec<ShardId> {
        self.multi_raft.shard_ids()
    }

    /// 获取指定分片的 Raft 状态
    pub fn shard_raft_state(&self, shard_id: ShardId) -> Option<RaftState> {
        self.multi_raft.raft_group(shard_id).map(|r| r.state())
    }

    /// 获取指定分片的 Leader 节点 ID（若本节点是 Leader）
    pub fn shard_leader(&self, shard_id: ShardId) -> Option<NodeId> {
        self.multi_raft.shard_leader(shard_id)
    }

    /// 同步 MultiRaftNode 内部状态机到本地缓存
    ///
    /// 注：MultiRaftNode::tick 已将 apply 的命令写入其内部状态机，
    /// 此处从其内部读取并同步到本地缓存（用于 get/scan 快速读取）。
    fn sync_state_machine(&mut self, shard_id: ShardId) {
        if let Some(sm) = self.multi_raft.state_machine(shard_id) {
            // 克隆内部状态（KvStateMachine 实现了 Clone）
            if let Some(local_sm) = self.state_machines.get_mut(&shard_id) {
                *local_sm = sm.clone();
            }
        }
    }

    /// 获取 KV 存储的键数量（用于测试和监控）
    pub fn kv_len(&self) -> Result<usize, DistRuntimeError> {
        let mut total = 0;
        for sid in self.shard_ids() {
            if let Some(sm) = self.multi_raft.state_machine(sid) {
                total += sm.len();
            }
        }
        Ok(total)
    }

    // ==================================================================
    //  P0-DIST 迭代 2：多节点集群支持方法
    // ==================================================================

    /// 推进 Multi-Raft 时钟并返回产生的 RPC 消息（多节点模式使用）
    ///
    /// 单节点模式下 `tick` 丢弃消息（无需跨节点投递）。
    /// 多节点模式下，调用方（DistCluster）需收集消息并投递到目标节点。
    ///
    /// # 参数
    /// - `elapsed_ms`：逻辑时间增量（毫秒）
    ///
    /// # 返回
    /// 需要投递到其他节点的 RPC 消息列表
    pub fn tick_with_messages(&mut self, elapsed_ms: u64) -> Vec<RpcMessage> {
        self.multi_raft.tick(elapsed_ms)
    }

    /// 处理收到的 RPC 消息（多节点模式使用）
    ///
    /// 由 DistCluster 在投递消息时调用，将消息分发到对应分片的 Raft 组。
    /// 返回处理过程中产生的响应消息（需继续投递回网络）。
    ///
    /// # 参数
    /// - `shard_id`：消息所属分片
    /// - `msg`：收到的 RPC 消息
    pub fn handle_message(&mut self, shard_id: ShardId, msg: RpcMessage) -> Vec<RpcMessage> {
        self.multi_raft.handle_message(shard_id, msg)
    }

    /// 仅 propose Put 命令，不推进 tick/apply（多节点模式使用）
    ///
    /// 多节点模式下，propose 后需通过 DistCluster::run_for 驱动 tick + 消息投递，
    /// 等待多数派复制后才会 commit/apply。此方法仅追加日志，不期望立即生效。
    pub fn propose_put_only(
        &mut self,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<Index, DistRuntimeError> {
        let shard_id = self.route(&key)?;
        let command = ShardCommand::Put {
            key: key.clone(),
            value,
        };
        self.propose(shard_id, command)
    }

    /// 仅 propose Delete 命令，不推进 tick/apply（多节点模式使用）
    pub fn propose_delete_only(&mut self, key: Vec<u8>) -> Result<Index, DistRuntimeError> {
        let shard_id = self.route(&key)?;
        let command = ShardCommand::Delete { key: key.clone() };
        self.propose(shard_id, command)
    }

    /// 推进 apply 并同步本地状态机缓存（多节点模式在消息投递后调用）
    ///
    /// 多节点模式下，commit_index 在 `handle_append_entries_response` 中推进，
    /// 但 apply 需要在下一次 tick 中完成。此方法手动推进 apply + 同步缓存，
    /// 供 DistCluster 在 deliver_all 后调用，确保读取到最新已提交数据。
    pub fn sync_apply(&mut self) {
        for sid in self.shard_ids() {
            self.multi_raft.advance_and_apply(sid);
            self.sync_state_machine(sid);
        }
    }

    /// 返回 runtime 是否已调用 `init()` 完成初始化（单节点自选举）
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// 判断本节点是否为指定分片的 Leader
    pub fn is_leader(&self) -> bool {
        for sid in self.shard_ids() {
            if self.multi_raft.shard_leader(sid).is_some() {
                return true;
            }
        }
        false
    }

    /// 获取本节点的 Raft 状态
    pub fn raft_state(&self) -> Option<RaftState> {
        self.shard_raft_state(1)
    }

    /// P0-DIST 迭代 2：标记节点为已初始化（多节点模式使用）
    ///
    /// 多节点模式下，不调用 `promote_to_leader`（那是单节点专用），
    /// 而是通过 `DistCluster::init` 驱动 tick + 消息投递触发自然选举。
    /// 此方法仅标记 `initialized = true`，跳过单节点的自选举逻辑。
    pub fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    /// P0-DIST 迭代 2：获取本节点第一个分片的当前 term
    ///
    /// 用于多节点集群中验证所有节点 term 一致性。
    pub fn current_term(&self) -> u64 {
        self.multi_raft
            .raft_group(1)
            .map(|r| r.current_term())
            .unwrap_or(0)
    }
}

impl std::fmt::Debug for DistRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DistRuntime")
            .field("node_id", &self.node_id)
            .field("shard_count", &self.multi_raft.shard_ids().len())
            .field("current_ts", &self.tso.current())
            .field("initialized", &self.initialized)
            .finish()
    }
}

// =====================================================================
//  DistRuntimeHandle — Arc 共享句柄
// =====================================================================

/// 分布式运行时的 Arc 共享句柄
///
/// 通过 `Arc<RwLock<DistRuntime>>` 包装，可安全跨线程共享。
/// 注入到 session/executor，提供分布式 KV 和事务能力。
///
/// # 用法
/// ```rust,ignore
/// let handle = DistRuntimeHandle::new_single_node(1)?;
/// handle.put(b"k1".to_vec(), b"v1".to_vec())?;
/// let v = handle.get(b"k1")?; // Some(b"v1")
/// ```
pub type DistRuntimeHandle = Arc<RwLock<DistRuntime>>;

/// 创建单节点 DistRuntime 的 Arc 共享句柄
pub fn new_single_node_runtime(node_id: NodeId) -> Result<DistRuntimeHandle, DistRuntimeError> {
    let runtime = DistRuntime::new_single_node(node_id)?;
    Ok(Arc::new(RwLock::new(runtime)))
}

/// P8-3：创建多节点集群中一个节点的 DistRuntime 共享句柄。
///
/// 与 `new_single_node_runtime` 不同：
/// - 分片 peers 包含所有节点（非仅自己），支持跨节点日志复制
/// - 不调用 `promote_to_leader`，由集群 tick 驱动自然选举
/// - 需配合 `ClusterDriver` 驱动 Raft tick + TCP 消息投递
///
/// # 参数
/// - `node_id`：本节点 ID
/// - `all_nodes`：集群所有节点 ID 列表（含本节点）
/// - `seed`：确定性随机种子（所有节点必须相同，确保选举超时可复现）
pub fn new_cluster_node_runtime(
    node_id: NodeId,
    all_nodes: &[NodeId],
    seed: u64,
) -> Result<DistRuntimeHandle, DistRuntimeError> {
    let runtime = DistRuntime::new_cluster_node(node_id, all_nodes, seed)?;
    Ok(Arc::new(RwLock::new(runtime)))
}

// =====================================================================
//  ClusterDriver — P8-3 多节点集群驱动器
// =====================================================================

/// P8-3：多节点集群驱动器。
///
/// 在后台线程中周期性驱动 Raft tick + TCP 消息投递，使多节点集群
/// 能够自然完成 Leader 选举和日志复制。
///
/// # 工作循环
/// 1. 调用 `DistRuntime::tick_with_messages()` 获取本节点产生的出站 RPC
/// 2. 通过 `TcpNetwork` 将出站消息发送到目标节点
/// 3. 从 `TcpNetwork::drain()` 获取收到的入站 RPC
/// 4. 调用 `DistRuntime::handle_message()` 处理入站消息
///
/// # 限制
/// - 当前仅支持单分片（shard_id=1），多分片扩展需在 wire format 中携带 shard_id
/// - tick 周期默认 50ms（与 Raft heartbeat_interval_ms 对齐）
pub struct ClusterDriver {
    runtime: DistRuntimeHandle,
    network: Arc<crate::network::TcpNetwork>,
    tick_interval_ms: u64,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    driver_thread: Option<std::thread::JoinHandle<()>>,
}

impl ClusterDriver {
    /// 创建集群驱动器。
    ///
    /// # 参数
    /// - `runtime`：DistRuntime 共享句柄
    /// - `network`：已启动监听的 TcpNetwork
    /// - `tick_interval_ms`：tick 周期（毫秒），建议 50ms
    pub fn new(
        runtime: DistRuntimeHandle,
        network: Arc<crate::network::TcpNetwork>,
        tick_interval_ms: u64,
    ) -> Self {
        Self {
            runtime,
            network,
            tick_interval_ms,
            stop_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            driver_thread: None,
        }
    }

    /// 启动后台驱动线程。
    ///
    /// 线程会持续运行直到 `stop()` 被调用。
    pub fn start(&mut self) -> Result<(), DistRuntimeError> {
        if self.driver_thread.is_some() {
            return Ok(()); // 已启动
        }

        let runtime = Arc::clone(&self.runtime);
        let network = Arc::clone(&self.network);
        let tick_ms = self.tick_interval_ms;
        let stop_flag = Arc::clone(&self.stop_flag);

        let handle = std::thread::Builder::new()
            .name("cluster-driver".to_string())
            .spawn(move || {
                tracing::info!(
                    tick_interval_ms = tick_ms,
                    "P8-3 ClusterDriver thread started"
                );
                while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                    // 1. tick 获取出站消息
                    let outbound = {
                        let mut rt = runtime.write();
                        rt.tick_with_messages(tick_ms)
                    };

                    // 2. 发送出站消息
                    for msg in outbound {
                        network.send(msg.from, msg.to, msg);
                    }

                    // 3. 获取入站消息
                    let inbound = network.drain();

                    // 4. 处理入站消息（当前仅 shard_id=1）
                    for msg in inbound {
                        let responses = {
                            let mut rt = runtime.write();
                            rt.handle_message(1, msg)
                        };
                        // 5. 发送处理后的响应
                        for resp in responses {
                            network.send(resp.from, resp.to, resp);
                        }
                    }

                    // 短暂休眠，避免 CPU 空转
                    std::thread::sleep(std::time::Duration::from_millis(tick_ms));
                }
                tracing::info!("P8-3 ClusterDriver thread stopped");
            })
            .map_err(|e| {
                DistRuntimeError::Serialization(format!(
                    "failed to spawn cluster driver thread: {e}"
                ))
            })?;

        self.driver_thread = Some(handle);
        Ok(())
    }

    /// 停止驱动线程。
    pub fn stop(&mut self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.driver_thread.take() {
            let _ = handle.join();
        }
    }

    /// 网络引用（用于外部查询监听地址等）
    pub fn network(&self) -> &Arc<crate::network::TcpNetwork> {
        &self.network
    }
}

impl Drop for ClusterDriver {
    fn drop(&mut self) {
        self.stop();
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-DIST-1：DistRuntime 单节点初始化 + Raft 自选举
    #[test]
    fn test_dist_runtime_init_single_node() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();
        assert_eq!(runtime.node_id(), 1);
        assert!(
            !runtime.shard_ids().is_empty(),
            "should have at least one shard"
        );
    }

    /// P0-DIST-1：单节点 Raft 写入 → apply → 读取
    #[test]
    fn test_dist_runtime_put_get() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 写入
        runtime.put(b"key1".to_vec(), b"value1".to_vec()).unwrap();
        // 读取
        let v = runtime.get(b"key1").unwrap();
        assert_eq!(v, Some(b"value1".to_vec()));
    }

    /// P0-DIST-1：多次写入 + 覆盖
    #[test]
    fn test_dist_runtime_put_overwrite() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        runtime.put(b"k".to_vec(), b"v1".to_vec()).unwrap();
        assert_eq!(runtime.get(b"k").unwrap(), Some(b"v1".to_vec()));

        runtime.put(b"k".to_vec(), b"v2".to_vec()).unwrap();
        assert_eq!(runtime.get(b"k").unwrap(), Some(b"v2".to_vec()));
    }

    /// P0-DIST-1：删除键
    #[test]
    fn test_dist_runtime_delete() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        runtime.put(b"k".to_vec(), b"v".to_vec()).unwrap();
        assert_eq!(runtime.get(b"k").unwrap(), Some(b"v".to_vec()));

        runtime.delete(b"k".to_vec()).unwrap();
        assert_eq!(runtime.get(b"k").unwrap(), None);
    }

    /// P0-DIST-2：TSO 时间戳单调递增
    #[test]
    fn test_dist_runtime_tso_monotonic() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let ts1 = runtime.begin_transaction();
        let ts2 = runtime.begin_transaction();
        let ts3 = runtime.begin_transaction();
        assert!(ts1 < ts2);
        assert!(ts2 < ts3);
    }

    /// P0-DIST-3：分片路由 — 所有键都路由到单分片
    #[test]
    fn test_dist_runtime_route_single_shard() {
        let runtime = DistRuntime::new_single_node(1).unwrap();
        let sid1 = runtime.route(b"any_key_1").unwrap();
        let sid2 = runtime.route(b"any_key_2").unwrap();
        let sid3 = runtime.route(b"another_key").unwrap();
        assert_eq!(sid1, sid2);
        assert_eq!(sid2, sid3);
    }

    /// P0-DIST-3：范围扫描
    #[test]
    fn test_dist_runtime_scan() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 写入多个键
        for i in 0..10 {
            let key = format!("key{:02}", i);
            let val = format!("val{}", i);
            runtime.put(key.into_bytes(), val.into_bytes()).unwrap();
        }

        // 扫描 [key03, key07)
        let range = KeyRange::new(b"key03".to_vec(), b"key07".to_vec());
        let results = runtime.scan(&range).unwrap();
        assert_eq!(
            results.len(),
            4,
            "should scan 4 keys [key03, key04, key05, key06]"
        );
        assert_eq!(results[0].0, b"key03");
        assert_eq!(results[3].0, b"key06");
    }

    /// P0-DIST-3：全范围扫描
    #[test]
    fn test_dist_runtime_scan_unbounded() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        runtime.put(b"a".to_vec(), b"1".to_vec()).unwrap();
        runtime.put(b"b".to_vec(), b"2".to_vec()).unwrap();
        runtime.put(b"c".to_vec(), b"3".to_vec()).unwrap();

        let range = KeyRange::unbounded();
        let results = runtime.scan(&range).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, b"a");
        assert_eq!(results[1].0, b"b");
        assert_eq!(results[2].0, b"c");
    }

    /// P0-DIST-1：KV 存储键计数
    #[test]
    fn test_dist_runtime_kv_len() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        assert_eq!(runtime.kv_len().unwrap(), 0);
        runtime.put(b"k1".to_vec(), b"v1".to_vec()).unwrap();
        runtime.put(b"k2".to_vec(), b"v2".to_vec()).unwrap();
        assert_eq!(runtime.kv_len().unwrap(), 2);
        runtime.delete(b"k1".to_vec()).unwrap();
        assert_eq!(runtime.kv_len().unwrap(), 1);
    }

    /// P0-DIST-1/2/3：端到端集成 — TSO + Raft + 分片路由
    #[test]
    fn test_dist_runtime_end_to_end_integration() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 1. TSO 分配事务时间戳
        let start_ts = runtime.begin_transaction();
        assert!(start_ts > 0);

        // 2. 路由键到分片
        let _shard_id = runtime.route(b"account:alice").unwrap();

        // 3. 通过 Raft propose 写入
        runtime
            .put(b"account:alice".to_vec(), b"1000".to_vec())
            .unwrap();
        runtime
            .put(b"account:bob".to_vec(), b"2000".to_vec())
            .unwrap();

        // 4. 读取验证
        let alice_balance = runtime.get(b"account:alice").unwrap();
        let bob_balance = runtime.get(b"account:bob").unwrap();
        assert_eq!(alice_balance, Some(b"1000".to_vec()));
        assert_eq!(bob_balance, Some(b"2000".to_vec()));

        // 5. 转账：alice -100, bob +100
        runtime
            .put(b"account:alice".to_vec(), b"900".to_vec())
            .unwrap();
        runtime
            .put(b"account:bob".to_vec(), b"2100".to_vec())
            .unwrap();

        // 6. 验证转账后余额
        assert_eq!(
            runtime.get(b"account:alice").unwrap(),
            Some(b"900".to_vec())
        );
        assert_eq!(runtime.get(b"account:bob").unwrap(), Some(b"2100".to_vec()));

        // 7. 提交时间戳
        let commit_ts = runtime.begin_transaction();
        assert!(commit_ts > start_ts);

        // 8. KV 存储状态
        assert_eq!(runtime.kv_len().unwrap(), 2);
    }

    /// DistRuntimeHandle（Arc 共享）跨线程读写
    #[test]
    fn test_dist_runtime_handle_shared() {
        let handle = new_single_node_runtime(1).unwrap();
        // 初始化
        {
            let mut rt = handle.write();
            rt.init().unwrap();
            rt.put(b"shared_key".to_vec(), b"shared_value".to_vec())
                .unwrap();
        }
        // 跨线程读取
        let handle_clone = handle.clone();
        let thread = std::thread::spawn(move || {
            let rt = handle_clone.read();
            rt.get(b"shared_key").unwrap()
        });
        let result = thread.join().unwrap();
        assert_eq!(result, Some(b"shared_value".to_vec()));
    }

    /// 100 个键的批量写入和扫描验证
    #[test]
    fn test_dist_runtime_batch_100_keys() {
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        // 写入 100 个键
        for i in 0..100 {
            let key = format!("k{:03}", i);
            let val = format!("v{:03}", i);
            runtime.put(key.into_bytes(), val.into_bytes()).unwrap();
        }

        // 验证总数
        assert_eq!(runtime.kv_len().unwrap(), 100);

        // 扫描 [k050, k060)
        let range = KeyRange::new(b"k050".to_vec(), b"k060".to_vec());
        let results = runtime.scan(&range).unwrap();
        assert_eq!(results.len(), 10);
        assert_eq!(results[0].0, b"k050");
        assert_eq!(results[9].0, b"k059");

        // 验证随机键
        assert_eq!(runtime.get(b"k042").unwrap(), Some(b"v042".to_vec()));
        assert_eq!(runtime.get(b"k099").unwrap(), Some(b"v099".to_vec()));
        assert_eq!(runtime.get(b"k100").unwrap(), None);
    }

    // ==================================================================
    //  P0-DIST 迭代 3：Multi-Raft 分片路由 + 跨分片查询测试
    // ==================================================================

    /// P0-DIST 迭代 3：多分片创建 + 路由验证
    #[test]
    fn test_multi_shard_routing() {
        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"m".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"m".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        // key "apple" → shard 1
        assert_eq!(runtime.route(b"apple").unwrap(), 1);
        // key "zebra" → shard 2
        assert_eq!(runtime.route(b"zebra").unwrap(), 2);
        // 边界 key "m" → shard 2（[start, end) 左闭右开）
        assert_eq!(runtime.route(b"m").unwrap(), 2);
        // key "a" → shard 1
        assert_eq!(runtime.route(b"a").unwrap(), 1);
        // key "l" → shard 1（"l" < "m"）
        assert_eq!(runtime.route(b"l").unwrap(), 1);

        // 分片数量验证
        assert_eq!(runtime.shard_count(), 2);
    }

    /// P0-DIST 迭代 3：多分片写入 + 读取（独立 Raft 组）
    #[test]
    fn test_multi_shard_put_get() {
        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"n".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"n".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        // 写入分片 1 的键
        runtime.put(b"apple".to_vec(), b"red".to_vec()).unwrap();
        runtime.put(b"banana".to_vec(), b"yellow".to_vec()).unwrap();
        // 写入分片 2 的键
        runtime.put(b"orange".to_vec(), b"orange".to_vec()).unwrap();
        runtime.put(b"zebra".to_vec(), b"black".to_vec()).unwrap();

        // 读取验证
        assert_eq!(runtime.get(b"apple").unwrap(), Some(b"red".to_vec()));
        assert_eq!(runtime.get(b"banana").unwrap(), Some(b"yellow".to_vec()));
        assert_eq!(runtime.get(b"orange").unwrap(), Some(b"orange".to_vec()));
        assert_eq!(runtime.get(b"zebra").unwrap(), Some(b"black".to_vec()));

        // 各分片键数量
        assert_eq!(runtime.shard_kv_len(1).unwrap(), 2);
        assert_eq!(runtime.shard_kv_len(2).unwrap(), 2);
        // 总键数
        assert_eq!(runtime.kv_len().unwrap(), 4);
    }

    /// P0-DIST 迭代 3：跨分片范围扫描
    #[test]
    fn test_multi_shard_cross_shard_scan() {
        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"n".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"n".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        // 写入跨分片的键
        for key in [b"a1", b"b2", b"c3", b"n4", b"o5", b"y6"] {
            runtime.put(key.to_vec(), key.to_vec()).unwrap();
        }

        // 跨分片扫描 [a, z) 应返回 6 个键（分片 1: a1,b2,c3；分片 2: n4,o5,y6）
        let range = KeyRange::new(b"a".to_vec(), b"z".to_vec());
        let results = runtime.scan(&range).unwrap();
        assert_eq!(results.len(), 6, "跨分片扫描应返回 6 个键");

        // 验证结果有序
        for i in 1..results.len() {
            assert!(results[i - 1].0 < results[i].0, "扫描结果应有序");
        }

        // 仅扫描分片 1 的范围 [a, n)
        let range1 = KeyRange::new(b"a".to_vec(), b"n".to_vec());
        let results1 = runtime.scan(&range1).unwrap();
        assert_eq!(results1.len(), 3, "分片 1 应有 3 个键");

        // 仅扫描分片 2 的范围 [n, +∞)
        let range2 = KeyRange::from(b"n".to_vec());
        let results2 = runtime.scan(&range2).unwrap();
        assert_eq!(results2.len(), 3, "分片 2 应有 3 个键");
    }

    /// P0-DIST 迭代 3：多分片删除验证
    #[test]
    fn test_multi_shard_delete() {
        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"n".to_vec()), vec![1]),
            Shard::new(2, KeyRange::from(b"n".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        // 写入
        runtime.put(b"apple".to_vec(), b"v1".to_vec()).unwrap();
        runtime.put(b"zebra".to_vec(), b"v2".to_vec()).unwrap();

        // 删除跨分片的键
        runtime.delete(b"apple".to_vec()).unwrap();
        runtime.delete(b"zebra".to_vec()).unwrap();

        // 验证已删除
        assert_eq!(runtime.get(b"apple").unwrap(), None);
        assert_eq!(runtime.get(b"zebra").unwrap(), None);
        assert_eq!(runtime.kv_len().unwrap(), 0);
    }

    /// P0-DIST 迭代 3：3 个分片验证
    #[test]
    fn test_multi_shard_three_shards() {
        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"i".to_vec()), vec![1]),
            Shard::new(2, KeyRange::new(b"i".to_vec(), b"r".to_vec()), vec![1]),
            Shard::new(3, KeyRange::from(b"r".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        assert_eq!(runtime.shard_count(), 3);

        // 写入各分片
        runtime.put(b"abc".to_vec(), b"s1".to_vec()).unwrap(); // shard 1
        runtime.put(b"jkl".to_vec(), b"s2".to_vec()).unwrap(); // shard 2
        runtime.put(b"xyz".to_vec(), b"s3".to_vec()).unwrap(); // shard 3

        // 验证路由
        assert_eq!(runtime.route(b"abc").unwrap(), 1);
        assert_eq!(runtime.route(b"jkl").unwrap(), 2);
        assert_eq!(runtime.route(b"xyz").unwrap(), 3);

        // 验证各分片独立
        assert_eq!(runtime.shard_kv_len(1).unwrap(), 1);
        assert_eq!(runtime.shard_kv_len(2).unwrap(), 1);
        assert_eq!(runtime.shard_kv_len(3).unwrap(), 1);

        // 验证读取
        assert_eq!(runtime.get(b"abc").unwrap(), Some(b"s1".to_vec()));
        assert_eq!(runtime.get(b"jkl").unwrap(), Some(b"s2".to_vec()));
        assert_eq!(runtime.get(b"xyz").unwrap(), Some(b"s3".to_vec()));

        // 跨分片扫描全部
        let all = runtime.scan(&KeyRange::unbounded()).unwrap();
        assert_eq!(all.len(), 3);
    }

    /// P0-DIST 迭代 3：动态添加分片
    #[test]
    fn test_multi_shard_add_shard_dynamically() {
        // 初始只有 1 个分片
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        assert_eq!(runtime.shard_count(), 1);

        // 写入初始数据
        runtime.put(b"key1".to_vec(), b"v1".to_vec()).unwrap();

        // 动态添加新分片（范围不与现有分片重叠）
        // 注：现有分片是无界的，所以新分片不会路由到（无界分片覆盖所有键）
        // 这里验证 add_shard 不会破坏现有功能
        let new_shard = Shard::new(2, KeyRange::new(b"z".to_vec(), b"zzz".to_vec()), vec![1]);
        runtime.add_shard(new_shard, 100).unwrap();

        assert_eq!(runtime.shard_count(), 2);

        // 原有数据仍可读
        assert_eq!(runtime.get(b"key1").unwrap(), Some(b"v1".to_vec()));
    }

    /// P0-DIST 迭代 3：多分片 + Percolator 事务集成
    ///
    /// 注：Percolator 使用前缀字节（0x01/0x02/0x03）编码键，这些前缀字节
    /// 会改变键的排序，导致基于原始键的范围分片无法正确路由带前缀的存储键。
    /// 因此 Percolator 事务在单分片（无界 KeyRange）上运行。
    /// 多分片 Percolator 需要基于原始键路由 + 分片级操作（未来工作）。
    #[test]
    fn test_multi_shard_with_percolator_txn() {
        use crate::dist_txn::DistTxnClient;
        use crate::txn::Mutation;

        // 使用单分片（无界），Percolator 事务在此上运行
        let mut runtime = DistRuntime::new_single_node(1).unwrap();
        runtime.init().unwrap();

        let mut txn = DistTxnClient::new(&mut runtime);
        let start_ts = txn.begin();

        // 写入多个键
        let mutations = vec![
            Mutation::put(b"alice".to_vec(), b"1000".to_vec()),
            Mutation::put(b"bob".to_vec(), b"500".to_vec()),
        ];
        txn.prewrite_all(&mutations, start_ts).unwrap();
        txn.commit(&mutations, start_ts).unwrap();

        // 读取验证
        let read_ts = txn.begin();
        assert_eq!(txn.get(b"alice", read_ts).unwrap(), Some(b"1000".to_vec()));
        assert_eq!(txn.get(b"bob", read_ts).unwrap(), Some(b"500".to_vec()));

        // 转账：alice -300, bob +300
        let ts2 = txn.begin();
        let transfer = vec![
            Mutation::put(b"alice".to_vec(), b"700".to_vec()),
            Mutation::put(b"bob".to_vec(), b"800".to_vec()),
        ];
        txn.prewrite_all(&transfer, ts2).unwrap();
        txn.commit(&transfer, ts2).unwrap();

        let read_ts2 = txn.begin();
        assert_eq!(txn.get(b"alice", read_ts2).unwrap(), Some(b"700".to_vec()));
        assert_eq!(txn.get(b"bob", read_ts2).unwrap(), Some(b"800".to_vec()));
    }

    /// P0-DIST 迭代 3：多分片批量写入 + 跨分片一致性扫描
    #[test]
    fn test_multi_shard_batch_consistency() {
        // 键格式为 "k000"-"k099"（4 字节），边界用 "k050"（4 字节）确保正确分割
        let shards = vec![
            Shard::new(
                1,
                KeyRange::new(b"k000".to_vec(), b"k050".to_vec()),
                vec![1],
            ),
            Shard::new(2, KeyRange::from(b"k050".to_vec()), vec![1]),
        ];
        let mut runtime = DistRuntime::new_multi_shard_single_node(1, shards, 42).unwrap();
        runtime.init().unwrap();

        // 写入 100 个键，跨两个分片
        for i in 0..100u8 {
            let key = format!("k{:03}", i);
            let val = format!("v{:03}", i);
            runtime.put(key.into_bytes(), val.into_bytes()).unwrap();
        }

        // 验证总键数
        assert_eq!(runtime.kv_len().unwrap(), 100);

        // 分片 1 应有 k000-k049（50 个）
        assert_eq!(runtime.shard_kv_len(1).unwrap(), 50);
        // 分片 2 应有 k050-k099（50 个）
        assert_eq!(runtime.shard_kv_len(2).unwrap(), 50);

        // 跨分片扫描全量，应有序
        let all = runtime.scan(&KeyRange::unbounded()).unwrap();
        assert_eq!(all.len(), 100);
        for i in 1..all.len() {
            assert!(all[i - 1].0 < all[i].0, "全量扫描应有序");
        }

        // 验证边界键
        assert_eq!(runtime.get(b"k049").unwrap(), Some(b"v049".to_vec()));
        assert_eq!(runtime.get(b"k050").unwrap(), Some(b"v050".to_vec()));
    }

    /// P8-3：验证 new_cluster_node_runtime 工厂函数
    #[test]
    fn test_new_cluster_node_runtime() {
        let all_nodes = vec![1, 2, 3];
        let handle = new_cluster_node_runtime(1, &all_nodes, 42).unwrap();
        let rt = handle.read();
        assert_eq!(rt.node_id(), 1);
        assert!(
            !rt.shard_ids().is_empty(),
            "cluster node should have shards"
        );
        // 多节点模式不应自动选举为 Leader（init() 才会）
        assert!(!rt.is_initialized());
    }

    /// P8-3：验证 ClusterDriver 创建与停止（不启动 TCP 监听，仅验证生命周期）
    #[test]
    fn test_cluster_driver_lifecycle() {
        let all_nodes = vec![1];
        let handle = new_cluster_node_runtime(1, &all_nodes, 42).unwrap();
        let network = Arc::new(crate::network::TcpNetwork::new(1));
        let mut driver = ClusterDriver::new(handle, network, 50);
        // 启动后立即停止，验证线程正常退出
        driver.start().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        driver.stop();
        // Drop 不应 panic
    }
}
