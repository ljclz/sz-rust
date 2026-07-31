//! Phase 8.4：Multi-Raft 分片（Range-based）
//!
//! 基于 Range 的分片策略：每个分片管理一段连续的键范围 [start, end)，
//! 每个分片由一个独立的 Raft 组管理。物理节点可同时参与多个 Raft 组。
//!
//! # 核心组件
//!
//! - [`KeyRange`] — 键范围 [start, end)，None 表示无界
//! - [`Shard`] — 分片元数据（id/range/raft 组成员）
//! - [`ShardRouter`] — 路由器，按键查找所属分片
//! - [`KvStateMachine`] — KV 状态机，应用 Put/Delete 命令
//! - [`ShardCommand`] — 分片命令编码（Put/Delete）
//! - [`MultiRaftNode`] — 物理节点，托管多个 Raft 组
//! - [`ShardCluster`] — 多分片集群测试夹具

use crate::raft::{
    Config, DEFAULT_SNAPSHOT_THRESHOLD, InMemoryNetwork, Index, MessageType, NodeId, RaftError,
    RaftNode, RpcMessage,
};
use std::collections::{BTreeMap, HashMap};
use tracing::{instrument, trace, warn};

// =====================================================================
//  类型别名
// =====================================================================

/// 分片 ID
pub type ShardId = u64;

// =====================================================================
//  KeyRange — 键范围 [start, end)
// =====================================================================

/// 键范围 [start, end)，None 表示无界（负/正无穷）。
///
/// 不变性：若 start 和 end 均为 Some，则 start < end。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRange {
    /// 起始键（包含），None 表示负无穷
    pub start: Option<Vec<u8>>,
    /// 结束键（不包含），None 表示正无穷
    pub end: Option<Vec<u8>>,
}

impl KeyRange {
    /// 创建无界范围 (-∞, +∞)
    pub fn unbounded() -> Self {
        Self {
            start: None,
            end: None,
        }
    }

    /// 创建 [start, end) 范围
    pub fn new(start: Vec<u8>, end: Vec<u8>) -> Self {
        Self {
            start: Some(start),
            end: Some(end),
        }
    }

    /// 创建 [start, +∞) 范围
    pub fn from(start: Vec<u8>) -> Self {
        Self {
            start: Some(start),
            end: None,
        }
    }

    /// 判断键是否落在本范围内
    pub fn contains(&self, key: &[u8]) -> bool {
        let ge_start = match &self.start {
            None => true,
            Some(s) => key >= s.as_slice(),
        };
        let lt_end = match &self.end {
            None => true,
            Some(e) => key < e.as_slice(),
        };
        ge_start && lt_end
    }

    /// 判断两个范围是否有重叠
    pub fn overlaps(&self, other: &KeyRange) -> bool {
        // self.start < other.end && other.start < self.end
        let self_lt_other_end = match (&self.end, &other.start) {
            (None, _) | (_, None) => true,
            (Some(e), Some(s)) => e > s,
        };
        let other_lt_self_end = match (&other.end, &self.start) {
            (None, _) | (_, None) => true,
            (Some(e), Some(s)) => e > s,
        };
        self_lt_other_end && other_lt_self_end
    }
}

// =====================================================================
//  Shard — 分片元数据
// =====================================================================

/// 分片元数据：一个键范围 + 管理该范围的 Raft 组成员列表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shard {
    /// 分片唯一 ID
    pub id: ShardId,
    /// 键范围
    pub range: KeyRange,
    /// Raft 组成员列表（节点 ID）
    pub peers: Vec<NodeId>,
}

impl Shard {
    /// 创建新分片
    pub fn new(id: ShardId, range: KeyRange, peers: Vec<NodeId>) -> Self {
        Self { id, range, peers }
    }

    /// 判断键是否属于本分片
    pub fn contains(&self, key: &[u8]) -> bool {
        self.range.contains(key)
    }
}

// =====================================================================
//  ShardRouter — 路由器
// =====================================================================

/// 分片路由器：维护按 start_key 排序的分片列表，支持按键和按范围路由。
///
/// 不变性：分片范围不重叠，且覆盖整个键空间（通过无界分片保证）。
#[derive(Clone, Debug, Default)]
pub struct ShardRouter {
    /// 按 id 索引的分片表
    shards: HashMap<ShardId, Shard>,
}

impl ShardRouter {
    /// 创建空路由器
    pub fn new() -> Self {
        Self {
            shards: HashMap::new(),
        }
    }

    /// 添加分片
    pub fn add_shard(&mut self, shard: Shard) {
        self.shards.insert(shard.id, shard);
    }

    /// 移除分片
    pub fn remove_shard(&mut self, id: ShardId) -> Option<Shard> {
        self.shards.remove(&id)
    }

    /// 获取所有分片
    pub fn shards(&self) -> &HashMap<ShardId, Shard> {
        &self.shards
    }

    /// 获取所有分片（可变引用，用于成员变更等场景）
    pub fn shards_mut(&mut self) -> &mut HashMap<ShardId, Shard> {
        &mut self.shards
    }

    /// 按键路由：返回键所属的分片 ID
    ///
    /// # Errors
    /// 若没有分片包含该键，返回 `RaftError::Other`。
    #[instrument(skip(self, key), fields(key_len = key.len(), shard_id = tracing::field::Empty))]
    pub fn route(&self, key: &[u8]) -> Result<ShardId, RaftError> {
        for (id, shard) in &self.shards {
            if shard.contains(key) {
                tracing::Span::current().record("shard_id", id);
                trace!(shard_id = id, "key routed to shard");
                return Ok(*id);
            }
        }
        warn!(key_len = key.len(), "no shard covers key");
        Err(RaftError::ConfigError(format!(
            "no shard covers key: {:?}",
            key
        )))
    }

    /// 按范围路由：返回与查询范围重叠的所有分片 ID
    pub fn route_range(&self, range: &KeyRange) -> Vec<ShardId> {
        self.shards
            .iter()
            .filter(|(_, s)| s.range.overlaps(range))
            .map(|(id, _)| *id)
            .collect()
    }

    /// 获取分片
    pub fn get_shard(&self, id: ShardId) -> Option<&Shard> {
        self.shards.get(&id)
    }
}

// =====================================================================
//  ShardCommand — 分片命令编码
// =====================================================================

/// 命令操作类型
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CmdType {
    /// 写入键值
    Put = 0x01,
    /// 删除键
    Delete = 0x02,
}

impl CmdType {
    /// 从字节解析
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Put),
            0x02 => Some(Self::Delete),
            _ => None,
        }
    }
}

/// 分片命令：Put / Delete
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShardCommand {
    /// 写入键值
    Put { key: Vec<u8>, value: Vec<u8> },
    /// 删除键
    Delete { key: Vec<u8> },
}

impl ShardCommand {
    /// 编码为字节序列（供 Raft propose）
    ///
    /// 格式：[cmd_type: u8][key_len: u32 BE][key][val_len: u32 BE][value]
    /// Delete 没有 value 部分（val_len=0）。
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Put { key, value } => {
                let mut buf = Vec::with_capacity(1 + 4 + key.len() + 4 + value.len());
                buf.push(CmdType::Put as u8);
                buf.extend_from_slice(&(key.len() as u32).to_be_bytes());
                buf.extend_from_slice(key);
                buf.extend_from_slice(&(value.len() as u32).to_be_bytes());
                buf.extend_from_slice(value);
                buf
            }
            Self::Delete { key } => {
                let mut buf = Vec::with_capacity(1 + 4 + key.len());
                buf.push(CmdType::Delete as u8);
                buf.extend_from_slice(&(key.len() as u32).to_be_bytes());
                buf.extend_from_slice(key);
                buf
            }
        }
    }

    /// 从字节序列解码
    ///
    /// # Errors
    /// 格式非法时返回 `RaftError::Other`。
    pub fn decode(data: &[u8]) -> Result<Self, RaftError> {
        if data.is_empty() {
            return Err(RaftError::ConfigError("empty command".into()));
        }
        let cmd_type = CmdType::from_byte(data[0]).ok_or_else(|| {
            RaftError::ConfigError(format!("unknown cmd type: 0x{:02X}", data[0]))
        })?;
        let mut pos = 1usize;

        // 读取 key
        if data.len() < pos + 4 {
            return Err(RaftError::ConfigError("truncated key length".into()));
        }
        let key_len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if data.len() < pos + key_len {
            return Err(RaftError::ConfigError("truncated key".into()));
        }
        let key = data[pos..pos + key_len].to_vec();
        pos += key_len;

        match cmd_type {
            CmdType::Put => {
                if data.len() < pos + 4 {
                    return Err(RaftError::ConfigError("truncated value length".into()));
                }
                let val_len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4;
                if data.len() < pos + val_len {
                    return Err(RaftError::ConfigError("truncated value".into()));
                }
                let value = data[pos..pos + val_len].to_vec();
                Ok(Self::Put { key, value })
            }
            CmdType::Delete => Ok(Self::Delete { key }),
        }
    }
}

// =====================================================================
//  KvStateMachine — KV 状态机
// =====================================================================

/// KV 状态机：应用 ShardCommand 后维护键值存储。
///
/// 使用 BTreeMap 以支持有序扫描。
#[derive(Clone, Debug, Default)]
pub struct KvStateMachine {
    /// 键值存储（有序）
    kv: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl KvStateMachine {
    /// 创建空状态机
    pub fn new() -> Self {
        Self {
            kv: BTreeMap::new(),
        }
    }

    /// 应用一条已提交的日志命令
    pub fn apply_command(&mut self, cmd: &ShardCommand) {
        match cmd {
            ShardCommand::Put { key, value } => {
                self.kv.insert(key.clone(), value.clone());
            }
            ShardCommand::Delete { key } => {
                self.kv.remove(key);
            }
        }
    }

    /// 从原始日志字节应用命令
    pub fn apply_raw(&mut self, data: &[u8]) {
        if let Ok(cmd) = ShardCommand::decode(data) {
            self.apply_command(&cmd);
        }
    }

    /// 读取单个键
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.kv.get(key).map(|v| v.as_slice())
    }

    /// 范围扫描 [start, end)，返回有序结果
    pub fn scan(&self, range: &KeyRange) -> Vec<(Vec<u8>, Vec<u8>)> {
        let iter: Box<dyn Iterator<Item = (&Vec<u8>, &Vec<u8>)>> = match &range.start {
            Some(start) => Box::new(self.kv.range(start.clone()..)),
            None => Box::new(self.kv.iter()),
        };
        iter.take_while(|(k, _)| match &range.end {
            Some(end) => k.as_slice() < end.as_slice(),
            None => true,
        })
        .filter(|(k, _)| range.contains(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
    }

    /// 返回键数量
    pub fn len(&self) -> usize {
        self.kv.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.kv.is_empty()
    }

    /// 返回内部引用（测试用）
    pub fn inner(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> {
        &self.kv
    }
}

// =====================================================================
//  MultiRaftNode — 物理节点，托管多个 Raft 组
// =====================================================================

/// 物理节点：一个节点可参与多个分片的 Raft 组。
///
/// 每个分片对应一个独立的 [`RaftNode`] 实例和 [`KvStateMachine`]。
/// 节点通过 [`ShardRouter`] 确定键属于哪个分片。
pub struct MultiRaftNode {
    /// 物理节点 ID
    pub node_id: NodeId,
    /// 分片路由器
    pub router: ShardRouter,
    /// 每个分片对应的 Raft 节点
    raft_groups: HashMap<ShardId, RaftNode>,
    /// 每个分片对应的 KV 状态机
    state_machines: HashMap<ShardId, KvStateMachine>,
}

impl MultiRaftNode {
    /// 创建物理节点
    pub fn new(node_id: NodeId, router: ShardRouter) -> Self {
        Self {
            node_id,
            router,
            raft_groups: HashMap::new(),
            state_machines: HashMap::new(),
        }
    }

    /// 为本节点加入一个分片的 Raft 组
    pub fn join_shard(&mut self, shard: &Shard, seed: u64) {
        let peers: Vec<NodeId> = shard
            .peers
            .iter()
            .copied()
            .filter(|&p| p != self.node_id)
            .collect();
        let config = Config {
            peers,
            election_timeout_min_ms: 150,
            election_timeout_max_ms: 300,
            heartbeat_interval_ms: 50,
            seed,
            snapshot_threshold: DEFAULT_SNAPSHOT_THRESHOLD,
        };
        let raft_node = RaftNode::new(self.node_id, config);
        self.raft_groups.insert(shard.id, raft_node);
        self.state_machines.insert(shard.id, KvStateMachine::new());
    }

    /// P0-DIST-1：单节点模式下，将指定分片的 Raft 组自选举为 Leader。
    ///
    /// 单节点配置（peers 为空）下，RaftNode 初始为 Follower，无法通过 tick
    /// 触发选举（无其他节点投票）。此方法直接调用 `become_candidate` +
    /// `become_leader`，使本节点成为该分片的 Leader，从而可以接受 propose。
    ///
    /// # Errors
    /// 分片不存在时返回 `RaftError::ConfigError`。
    pub fn promote_to_leader(&mut self, shard_id: ShardId) -> Result<(), RaftError> {
        let raft = self
            .raft_groups
            .get_mut(&shard_id)
            .ok_or_else(|| RaftError::ConfigError(format!("shard {} not found", shard_id)))?;
        raft.become_candidate();
        raft.become_leader();
        Ok(())
    }

    /// 推进所有 Raft 组的时钟，返回产生的 RPC 消息
    pub fn tick(&mut self, elapsed_ms: u64) -> Vec<RpcMessage> {
        let mut messages = Vec::new();
        for (shard_id, raft_node) in &mut self.raft_groups {
            let msgs = raft_node.tick(elapsed_ms);
            // 标注消息所属分片（复用 RaftNode 的消息，shard_id 由调用方推断）
            for msg in msgs {
                messages.push(msg);
            }
            // 推进 apply
            let applied = raft_node.apply();
            let sm = self
                .state_machines
                .get_mut(shard_id)
                .expect("state machine must exist");
            for entry in applied {
                sm.apply_raw(&entry.command);
            }
        }
        messages
    }

    /// 向指定分片提议写入命令
    ///
    /// # Errors
    /// 非Leader 或 Raft 错误时返回。
    #[instrument(skip(self, command), fields(shard_id, cmd_type = ?command))]
    pub fn propose(
        &mut self,
        shard_id: ShardId,
        command: ShardCommand,
    ) -> Result<Index, RaftError> {
        tracing::Span::current().record("shard_id", shard_id);
        let raft = self
            .raft_groups
            .get_mut(&shard_id)
            .ok_or_else(|| RaftError::ConfigError(format!("shard {} not found", shard_id)))?;
        let result = raft.propose(command.encode());
        match &result {
            Ok(idx) => trace!(shard_id, index = idx, "command proposed"),
            Err(e) => warn!(shard_id, error = %e, "propose failed"),
        }
        // P0-DIST-1：单节点模式下，propose 后立即推进 commit + apply
        // 多节点模式下，commit 需等待 AppendEntriesResponse，由 tick 触发
        raft.advance_commit();
        let applied = raft.apply();
        let sm = self
            .state_machines
            .get_mut(&shard_id)
            .expect("state machine must exist");
        for entry in applied {
            sm.apply_raw(&entry.command);
        }
        result
    }

    /// P0-DIST-1：推进指定分片的 commit + apply（用于 tick 后手动触发）
    ///
    /// 在多节点模式下，`tick` 发送 AppendEntries 后需等待 Response，
    /// 调用此方法处理返回的 Response 后推进 commit。
    pub fn advance_and_apply(&mut self, shard_id: ShardId) {
        let Some(raft) = self.raft_groups.get_mut(&shard_id) else {
            return;
        };
        raft.advance_commit();
        let applied = raft.apply();
        let sm = self
            .state_machines
            .get_mut(&shard_id)
            .expect("state machine must exist");
        for entry in applied {
            sm.apply_raw(&entry.command);
        }
    }

    /// 处理收到的 RPC 消息（分发到对应分片的 Raft 组）
    pub fn handle_message(&mut self, shard_id: ShardId, msg: RpcMessage) -> Vec<RpcMessage> {
        let raft = match self.raft_groups.get_mut(&shard_id) {
            Some(r) => r,
            None => return Vec::new(),
        };
        match msg.message_type {
            MessageType::RequestVoteRequest(req) => {
                let resp = raft.handle_request_vote(req);
                vec![RpcMessage::new(
                    msg.to,
                    msg.from,
                    MessageType::RequestVoteResponse(resp),
                )]
            }
            MessageType::AppendEntriesRequest(req) => {
                let resp = raft.handle_append_entries(req);
                vec![RpcMessage::new(
                    msg.to,
                    msg.from,
                    MessageType::AppendEntriesResponse(resp),
                )]
            }
            MessageType::RequestVoteResponse(resp) => {
                raft.handle_request_vote_response(msg.from, resp)
            }
            MessageType::AppendEntriesResponse(resp) => {
                raft.handle_append_entries_response(msg.from, resp)
            }
        }
    }

    /// 获取分片的 Leader 节点 ID（若本节点是 Leader）
    pub fn shard_leader(&self, shard_id: ShardId) -> Option<NodeId> {
        let raft = self.raft_groups.get(&shard_id)?;
        if raft.state() == crate::raft::RaftState::Leader {
            Some(self.node_id)
        } else {
            None
        }
    }

    /// 获取分片的状态机
    pub fn state_machine(&self, shard_id: ShardId) -> Option<&KvStateMachine> {
        self.state_machines.get(&shard_id)
    }

    /// 获取分片的 Raft 节点引用
    pub fn raft_group(&self, shard_id: ShardId) -> Option<&RaftNode> {
        self.raft_groups.get(&shard_id)
    }

    /// 获取本节点参与的所有分片 ID
    pub fn shard_ids(&self) -> Vec<ShardId> {
        self.raft_groups.keys().copied().collect()
    }
}

// =====================================================================
//  带分片标注的消息
// =====================================================================

/// 带 ShardId 标注的 RPC 消息，用于多分片网络传输
#[derive(Clone, Debug)]
pub struct ShardedMessage {
    /// 目标分片
    pub shard_id: ShardId,
    /// 内部 RPC 消息
    pub inner: RpcMessage,
}

// =====================================================================
//  ShardNetwork — 多分片内存网络
// =====================================================================

/// 多分片内存网络：支持按物理节点离线/分区
#[derive(Default)]
pub struct ShardNetwork {
    /// 底层物理网络（共享，所有分片复用）
    inner: InMemoryNetwork,
    /// 待投递的带分片标注消息
    pending: Vec<ShardedMessage>,
}

impl ShardNetwork {
    /// 创建网络
    pub fn new() -> Self {
        Self {
            inner: InMemoryNetwork::new(),
            pending: Vec::new(),
        }
    }

    /// 设置节点离线
    pub fn set_offline(&self, node: NodeId) {
        self.inner.set_offline(node);
    }

    /// 设置节点上线
    pub fn set_online(&self, node: NodeId) {
        self.inner.set_online(node);
    }

    /// 分区两个节点
    pub fn partition(&self, a: NodeId, b: NodeId) {
        self.inner.partition(a, b);
    }

    /// 恢复所有分区
    pub fn heal_all(&self) {
        self.inner.heal_all();
    }

    /// 判断节点是否离线
    pub fn is_offline(&self, node: NodeId) -> bool {
        self.inner.is_offline(node)
    }

    /// 发送带分片标注的消息
    pub fn send(&mut self, shard_id: ShardId, msg: RpcMessage) {
        // 物理网络仅用于离线/分区判断，消息暂存到 pending
        if self.inner.is_offline(msg.from) || self.inner.is_offline(msg.to) {
            return;
        }
        if self.inner.is_partitioned(msg.from, msg.to) {
            return;
        }
        self.pending.push(ShardedMessage {
            shard_id,
            inner: msg,
        });
    }

    /// 取出所有待投递消息
    pub fn drain(&mut self) -> Vec<ShardedMessage> {
        std::mem::take(&mut self.pending)
    }

    /// 判断两个节点是否被分区
    pub fn is_partitioned(&self, a: NodeId, b: NodeId) -> bool {
        self.inner.is_partitioned(a, b)
    }
}

// =====================================================================
//  Phase 8.6：分片元数据管理
// =====================================================================

/// 元数据 Raft 组使用的保留 shard_id（不用于数据分片）
pub const META_SHARD_ID: ShardId = 0;

/// 编码 Option<Vec<u8>> 到缓冲区（None 用 0xFFFFFFFF 表示）
fn encode_opt_vec(buf: &mut Vec<u8>, opt: &Option<Vec<u8>>) {
    match opt {
        Some(v) => {
            buf.extend_from_slice(&(v.len() as u32).to_be_bytes());
            buf.extend_from_slice(v);
        }
        None => {
            buf.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
        }
    }
}

/// 从字节序列解码 Option<Vec<u8>>
fn decode_opt_vec(data: &[u8], pos: &mut usize) -> Result<Option<Vec<u8>>, RaftError> {
    if data.len() < *pos + 4 {
        return Err(RaftError::ConfigError("truncated opt_vec length".into()));
    }
    let len = u32::from_be_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    if len == 0xFFFFFFFF {
        return Ok(None);
    }
    let len = len as usize;
    if data.len() < *pos + len {
        return Err(RaftError::ConfigError("truncated opt_vec data".into()));
    }
    let v = data[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(Some(v))
}

/// 编码 Vec<NodeId> 到缓冲区
fn encode_node_ids(buf: &mut Vec<u8>, peers: &[NodeId]) {
    buf.extend_from_slice(&(peers.len() as u32).to_be_bytes());
    for &p in peers {
        buf.extend_from_slice(&p.to_be_bytes());
    }
}

/// 从字节序列解码 Vec<NodeId>
fn decode_node_ids(data: &[u8], pos: &mut usize) -> Result<Vec<NodeId>, RaftError> {
    if data.len() < *pos + 4 {
        return Err(RaftError::ConfigError("truncated node_ids count".into()));
    }
    let count = u32::from_be_bytes(data[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        if data.len() < *pos + 8 {
            return Err(RaftError::ConfigError("truncated node_id".into()));
        }
        result.push(u64::from_be_bytes(data[*pos..*pos + 8].try_into().unwrap()));
        *pos += 8;
    }
    Ok(result)
}

/// 元数据操作类型：记录分片路由表的变更。
///
/// 所有操作通过元数据 Raft 组复制，节点崩溃后可从日志重放重建路由器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataOp {
    /// 添加分片
    AddShard(Shard),
    /// 移除分片
    RemoveShard(ShardId),
    /// 更新分片 peers（用于迁移）
    UpdatePeers {
        /// 分片 ID
        shard_id: ShardId,
        /// 新的 peers 列表
        new_peers: Vec<NodeId>,
    },
    /// 分片分裂
    SplitShard {
        /// 原分片 ID
        old_id: ShardId,
        /// 新分片 1 ID
        new_id_1: ShardId,
        /// 新分片 2 ID
        new_id_2: ShardId,
        /// 分裂键
        split_key: Vec<u8>,
        /// 原分片范围
        original_range: KeyRange,
        /// 原分片 peers
        peers: Vec<NodeId>,
    },
}

impl MetadataOp {
    /// 编码为字节序列（供 Raft propose）
    ///
    /// # 格式
    /// - AddShard: [0x01][shard_id:u64][range_start:opt_vec][range_end:opt_vec][peers:node_ids]
    /// - RemoveShard: [0x02][shard_id:u64]
    /// - UpdatePeers: [0x03][shard_id:u64][new_peers:node_ids]
    /// - SplitShard: [0x04][old_id:u64][new_id_1:u64][new_id_2:u64][split_key:opt_vec][original_range:start+end][peers:node_ids]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        match self {
            Self::AddShard(shard) => {
                buf.push(0x01);
                buf.extend_from_slice(&shard.id.to_be_bytes());
                encode_opt_vec(&mut buf, &shard.range.start);
                encode_opt_vec(&mut buf, &shard.range.end);
                encode_node_ids(&mut buf, &shard.peers);
            }
            Self::RemoveShard(id) => {
                buf.push(0x02);
                buf.extend_from_slice(&id.to_be_bytes());
            }
            Self::UpdatePeers {
                shard_id,
                new_peers,
            } => {
                buf.push(0x03);
                buf.extend_from_slice(&shard_id.to_be_bytes());
                encode_node_ids(&mut buf, new_peers);
            }
            Self::SplitShard {
                old_id,
                new_id_1,
                new_id_2,
                split_key,
                original_range,
                peers,
            } => {
                buf.push(0x04);
                buf.extend_from_slice(&old_id.to_be_bytes());
                buf.extend_from_slice(&new_id_1.to_be_bytes());
                buf.extend_from_slice(&new_id_2.to_be_bytes());
                encode_opt_vec(&mut buf, &Some(split_key.clone()));
                encode_opt_vec(&mut buf, &original_range.start);
                encode_opt_vec(&mut buf, &original_range.end);
                encode_node_ids(&mut buf, peers);
            }
        }
        buf
    }

    /// 从字节序列解码
    ///
    /// # Errors
    /// 格式非法时返回 `RaftError::ConfigError`。
    pub fn decode(data: &[u8]) -> Result<Self, RaftError> {
        if data.is_empty() {
            return Err(RaftError::ConfigError("empty metadata op".into()));
        }
        let mut pos = 1usize;
        match data[0] {
            0x01 => {
                if data.len() < pos + 8 {
                    return Err(RaftError::ConfigError("truncated shard_id".into()));
                }
                let id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                let start = decode_opt_vec(data, &mut pos)?;
                let end = decode_opt_vec(data, &mut pos)?;
                let peers = decode_node_ids(data, &mut pos)?;
                Ok(Self::AddShard(Shard::new(
                    id,
                    KeyRange { start, end },
                    peers,
                )))
            }
            0x02 => {
                if data.len() < pos + 8 {
                    return Err(RaftError::ConfigError("truncated shard_id".into()));
                }
                let id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
                Ok(Self::RemoveShard(id))
            }
            0x03 => {
                if data.len() < pos + 8 {
                    return Err(RaftError::ConfigError("truncated shard_id".into()));
                }
                let shard_id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                let new_peers = decode_node_ids(data, &mut pos)?;
                Ok(Self::UpdatePeers {
                    shard_id,
                    new_peers,
                })
            }
            0x04 => {
                if data.len() < pos + 24 {
                    return Err(RaftError::ConfigError("truncated split shard ids".into()));
                }
                let old_id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                let new_id_1 = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                let new_id_2 = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
                pos += 8;
                let split_key = decode_opt_vec(data, &mut pos)?
                    .ok_or_else(|| RaftError::ConfigError("split_key must not be None".into()))?;
                let start = decode_opt_vec(data, &mut pos)?;
                let end = decode_opt_vec(data, &mut pos)?;
                let peers = decode_node_ids(data, &mut pos)?;
                Ok(Self::SplitShard {
                    old_id,
                    new_id_1,
                    new_id_2,
                    split_key,
                    original_range: KeyRange { start, end },
                    peers,
                })
            }
            _ => Err(RaftError::ConfigError(format!(
                "unknown metadata op type: 0x{:02X}",
                data[0]
            ))),
        }
    }
}

/// 元数据状态机：应用 MetadataOp 后维护 ShardRouter。
///
/// 节点崩溃恢复后，从元数据 Raft 组的已提交日志重放 MetadataOp，
/// 即可重建完整的分片路由表。
#[derive(Clone, Debug, Default)]
pub struct MetadataStateMachine {
    /// 分片路由器
    router: ShardRouter,
}

impl MetadataStateMachine {
    /// 创建空元数据状态机
    pub fn new() -> Self {
        Self {
            router: ShardRouter::new(),
        }
    }

    /// 应用一条元数据操作
    pub fn apply(&mut self, op: &MetadataOp) {
        match op {
            MetadataOp::AddShard(shard) => {
                self.router.add_shard(shard.clone());
            }
            MetadataOp::RemoveShard(id) => {
                self.router.remove_shard(*id);
            }
            MetadataOp::UpdatePeers {
                shard_id,
                new_peers,
            } => {
                if let Some(shard) = self.router.shards_mut().get_mut(shard_id) {
                    shard.peers = new_peers.clone();
                }
            }
            MetadataOp::SplitShard {
                old_id,
                new_id_1,
                new_id_2,
                split_key,
                original_range,
                peers,
            } => {
                let range1 = KeyRange {
                    start: original_range.start.clone(),
                    end: Some(split_key.clone()),
                };
                let range2 = KeyRange {
                    start: Some(split_key.clone()),
                    end: original_range.end.clone(),
                };
                self.router
                    .add_shard(Shard::new(*new_id_1, range1, peers.clone()));
                self.router
                    .add_shard(Shard::new(*new_id_2, range2, peers.clone()));
                self.router.remove_shard(*old_id);
            }
        }
    }

    /// 从原始日志字节应用操作
    pub fn apply_raw(&mut self, data: &[u8]) {
        if let Ok(op) = MetadataOp::decode(data) {
            self.apply(&op);
        }
    }

    /// 获取重建的路由器
    pub fn router(&self) -> &ShardRouter {
        &self.router
    }

    /// 返回已记录的分片数量
    pub fn shard_count(&self) -> usize {
        self.router.shards().len()
    }
}

// =====================================================================
//  ShardCluster — 多分片集群测试夹具
// =====================================================================

/// 多分片集群：N 个物理节点，每个分片的 Raft 组跨多个节点。
///
/// 用于集成测试：验证分片路由、数据写入、跨分片查询聚合。
/// Phase 8.6 新增：元数据 Raft 组管理分片路由表，节点崩溃后可从日志恢复。
pub struct ShardCluster {
    /// 物理节点集合
    pub nodes: HashMap<NodeId, MultiRaftNode>,
    /// 多分片网络
    pub network: ShardNetwork,
    /// 路由器（所有节点共享同一份路由表）
    pub router: ShardRouter,
    /// 下一个可用分片 ID（用于分裂时生成新 ID）
    next_shard_id: ShardId,
    /// 元数据 Raft 组（每个节点一个实例，shard_id=META_SHARD_ID）
    meta_rafts: HashMap<NodeId, RaftNode>,
    /// 元数据状态机（每个节点一个实例）
    meta_sms: HashMap<NodeId, MetadataStateMachine>,
}

impl ShardCluster {
    /// 创建多分片集群
    ///
    /// # Arguments
    /// * `node_ids` - 物理节点 ID 列表
    /// * `shards` - 分片定义列表
    /// * `seed` - 随机种子
    pub fn new(node_ids: &[NodeId], shards: Vec<Shard>, seed: u64) -> Self {
        let network = ShardNetwork::new();
        let mut router = ShardRouter::new();
        let mut nodes = HashMap::new();
        let mut meta_rafts = HashMap::new();
        let mut meta_sms = HashMap::new();

        // 注册分片并计算 next_shard_id
        let mut max_id = 0u64;
        for shard in &shards {
            router.add_shard(shard.clone());
            if shard.id > max_id {
                max_id = shard.id;
            }
        }

        // 创建物理节点并加入其参与的分片
        for &nid in node_ids {
            let mut node = MultiRaftNode::new(nid, router.clone());
            for shard in &shards {
                if shard.peers.contains(&nid) {
                    node.join_shard(shard, seed + nid);
                }
            }
            nodes.insert(nid, node);

            // Phase 8.6：初始化元数据 Raft 组（所有节点参与）
            let meta_peers: Vec<NodeId> = node_ids.iter().copied().filter(|&p| p != nid).collect();
            let meta_config = Config {
                peers: meta_peers,
                election_timeout_min_ms: 150,
                election_timeout_max_ms: 300,
                heartbeat_interval_ms: 50,
                seed: seed + nid + 99999,
                snapshot_threshold: DEFAULT_SNAPSHOT_THRESHOLD,
            };
            meta_rafts.insert(nid, RaftNode::new(nid, meta_config));
            let mut meta_sm = MetadataStateMachine::new();
            // 将初始分片记录到元数据状态机
            for shard in &shards {
                meta_sm.apply(&MetadataOp::AddShard(shard.clone()));
            }
            meta_sms.insert(nid, meta_sm);
        }

        Self {
            nodes,
            network,
            router,
            next_shard_id: max_id + 1,
            meta_rafts,
            meta_sms,
        }
    }

    /// 分配下一个分片 ID
    fn alloc_shard_id(&mut self) -> ShardId {
        let id = self.next_shard_id;
        self.next_shard_id += 1;
        id
    }

    /// 分裂分片：将指定分片在 split_key 处分裂为两个新分片。
    ///
    /// 流程：
    /// 1. 读取原分片 Leader 的全部数据
    /// 2. 创建两个新分片 [start, split_key) 和 [split_key, end)
    /// 3. 在原分片的物理节点上创建新 Raft 组
    /// 4. 等待新 Leader 选举
    /// 5. 将数据写入对应的新分片
    /// 6. 更新路由器（移除旧分片，添加两个新分片）
    ///
    /// # Errors
    /// 分片不存在、无 Leader 或写入失败时返回。
    pub fn split_shard(
        &mut self,
        shard_id: ShardId,
        split_key: Vec<u8>,
        seed: u64,
    ) -> Result<(ShardId, ShardId), RaftError> {
        // 1. 获取原分片信息
        let original = self
            .router
            .get_shard(shard_id)
            .ok_or_else(|| RaftError::ConfigError(format!("shard {} not found", shard_id)))?
            .clone();

        // 2. 读取原分片 Leader 的全部数据
        let leader = self
            .shard_leader(shard_id)
            .ok_or_else(|| RaftError::ConfigError(format!("no leader for shard {}", shard_id)))?;
        let all_data: Vec<(Vec<u8>, Vec<u8>)> = self
            .nodes
            .get(&leader)
            .and_then(|n| n.state_machine(shard_id))
            .map(|sm| sm.scan(&KeyRange::unbounded()))
            .unwrap_or_default();

        // 3. 创建两个新分片
        let new_id_1 = self.alloc_shard_id();
        let new_id_2 = self.alloc_shard_id();
        let split_key_clone = split_key.clone();
        let range1 = KeyRange {
            start: original.range.start.clone(),
            end: Some(split_key.clone()),
        };
        let range2 = KeyRange {
            start: Some(split_key),
            end: original.range.end.clone(),
        };
        let shard1 = Shard::new(new_id_1, range1, original.peers.clone());
        let shard2 = Shard::new(new_id_2, range2, original.peers.clone());

        // 4. 在原分片的物理节点上移除旧 Raft 组并创建新 Raft 组
        for &nid in &original.peers {
            if let Some(node) = self.nodes.get_mut(&nid) {
                // 数据已读取，安全移除旧分片的 Raft 组和状态机
                node.leave_shard(shard_id);
                node.router = self.router.clone();
                node.join_shard(&shard1, seed + nid);
                node.join_shard(&shard2, seed + nid + 10000);
            }
        }

        // 5. 更新路由器（先添加新分片，再移除旧分片）
        self.router.add_shard(shard1);
        self.router.add_shard(shard2);
        self.router.remove_shard(shard_id);

        // 6. 同步路由器到所有节点
        for node in self.nodes.values_mut() {
            node.router = self.router.clone();
        }

        // 7. 等待新 Leader 选举
        self.run_for(1500);

        // 8. 将数据写入对应的新分片
        for (key, value) in all_data {
            let target = if self.router.route(&key)? == new_id_1 {
                new_id_1
            } else {
                new_id_2
            };
            self.put(target, key, value)?;
            self.run_for(100);
        }
        self.run_for(500);

        // Phase 8.6：将分裂操作记录到元数据 Raft 组
        let _ = self.propose_metadata(MetadataOp::SplitShard {
            old_id: shard_id,
            new_id_1,
            new_id_2,
            split_key: split_key_clone,
            original_range: original.range.clone(),
            peers: original.peers.clone(),
        });

        Ok((new_id_1, new_id_2))
    }

    /// 迁移分片：将分片的 Raft 组成员从当前配置变更为 new_peers。
    ///
    /// 流程：
    /// 1. 新节点 join_shard 加入分片
    /// 2. Leader 调用 propose_membership_change_v2 发起联合共识
    /// 3. 等待变更完成
    /// 4. 被移除的节点 leave_shard
    /// 5. 更新路由器中的分片 peers
    ///
    /// # Errors
    /// 分片不存在、无 Leader 或成员变更失败时返回。
    pub fn migrate_shard(
        &mut self,
        shard_id: ShardId,
        new_peers: Vec<NodeId>,
        seed: u64,
    ) -> Result<(), RaftError> {
        // 1. 获取原分片信息
        let original = self
            .router
            .get_shard(shard_id)
            .ok_or_else(|| RaftError::ConfigError(format!("shard {} not found", shard_id)))?
            .clone();

        // 2. 新节点 join_shard
        for &nid in &new_peers {
            if !original.peers.contains(&nid) {
                if let Some(node) = self.nodes.get_mut(&nid) {
                    let shard = Shard::new(shard_id, original.range.clone(), new_peers.clone());
                    node.join_shard(&shard, seed + nid);
                }
            }
        }

        // 3. Leader 发起成员变更
        let leader = self
            .shard_leader(shard_id)
            .ok_or_else(|| RaftError::ConfigError(format!("no leader for shard {}", shard_id)))?;
        let node = self.nodes.get_mut(&leader).unwrap();
        let raft = node.raft_group_mut(shard_id).unwrap();
        raft.propose_membership_change_v2(new_peers.clone())?;

        // 4. 等待变更完成
        self.run_for(2000);

        // 5. 被移除的节点 leave_shard
        for &nid in &original.peers {
            if !new_peers.contains(&nid) {
                if let Some(node) = self.nodes.get_mut(&nid) {
                    node.leave_shard(shard_id);
                }
            }
        }

        // 6. 更新路由器中的分片 peers
        let new_peers_clone = new_peers.clone();
        if let Some(shard) = self.router.shards_mut().get_mut(&shard_id) {
            shard.peers = new_peers;
        }

        // 7. 同步路由器到所有节点
        for node in self.nodes.values_mut() {
            node.router = self.router.clone();
        }

        self.run_for(500);

        // Phase 8.6：将迁移操作记录到元数据 Raft 组
        let _ = self.propose_metadata(MetadataOp::UpdatePeers {
            shard_id,
            new_peers: new_peers_clone,
        });

        Ok(())
    }

    /// 推进所有节点的所有分片 Raft 时钟 + 元数据 Raft 时钟
    fn tick_sharded(&mut self, ms: u64) {
        // 收集需要 tick 的节点 ID（避免借用冲突）
        let offline_nodes: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|&&nid| self.network.is_offline(nid))
            .copied()
            .collect();

        // 推进数据分片 Raft 组
        for (&nid, node) in &mut self.nodes {
            if offline_nodes.contains(&nid) {
                continue;
            }
            for shard_id in node.shard_ids() {
                let msgs = node.tick_shard(shard_id, ms);
                for msg in msgs {
                    self.network.send(shard_id, msg);
                }
            }
        }

        // Phase 8.6：推进元数据 Raft 组
        for (&nid, meta_raft) in &mut self.meta_rafts {
            if offline_nodes.contains(&nid) {
                continue;
            }
            let msgs = meta_raft.tick(ms);
            for msg in msgs {
                self.network.send(META_SHARD_ID, msg);
            }
            // 推进 apply 到元数据状态机
            let applied = meta_raft.apply();
            let meta_sm = self
                .meta_sms
                .get_mut(&nid)
                .expect("meta state machine must exist");
            for entry in applied {
                meta_sm.apply_raw(&entry.command);
            }
        }
    }

    /// 投递所有待处理消息（最多 200 轮），元数据消息路由到 meta Raft 组
    fn deliver_all(&mut self) {
        for _ in 0..200 {
            let messages = self.network.drain();
            if messages.is_empty() {
                break;
            }
            for smsg in messages {
                // Phase 8.6：元数据消息路由到 meta_rafts
                if smsg.shard_id == META_SHARD_ID {
                    if let Some(meta_raft) = self.meta_rafts.get_mut(&smsg.inner.to) {
                        let responses: Vec<RpcMessage> = match smsg.inner.message_type {
                            MessageType::RequestVoteRequest(req) => {
                                let resp = meta_raft.handle_request_vote(req);
                                vec![RpcMessage::new(
                                    smsg.inner.to,
                                    smsg.inner.from,
                                    MessageType::RequestVoteResponse(resp),
                                )]
                            }
                            MessageType::AppendEntriesRequest(req) => {
                                let resp = meta_raft.handle_append_entries(req);
                                vec![RpcMessage::new(
                                    smsg.inner.to,
                                    smsg.inner.from,
                                    MessageType::AppendEntriesResponse(resp),
                                )]
                            }
                            MessageType::RequestVoteResponse(resp) => {
                                meta_raft.handle_request_vote_response(smsg.inner.from, resp)
                            }
                            MessageType::AppendEntriesResponse(resp) => {
                                meta_raft.handle_append_entries_response(smsg.inner.from, resp)
                            }
                        };
                        for resp in responses {
                            self.network.send(META_SHARD_ID, resp);
                        }
                    }
                } else if let Some(target) = self.nodes.get_mut(&smsg.inner.to) {
                    let responses = target.handle_message(smsg.shard_id, smsg.inner);
                    for resp in responses {
                        self.network.send(smsg.shard_id, resp);
                    }
                }
            }
        }
    }

    /// 运行指定逻辑时间（步进 10ms）
    pub fn run_for(&mut self, total_ms: u64) {
        let step = 10u64;
        let mut elapsed = 0u64;
        while elapsed < total_ms {
            self.tick_sharded(step);
            self.deliver_all();
            elapsed += step;
        }
    }

    /// 查找指定分片的 Leader 节点（跳过离线节点）
    pub fn shard_leader(&self, shard_id: ShardId) -> Option<NodeId> {
        for (nid, node) in &self.nodes {
            // 离线节点的 Raft 状态可能仍为 Leader，但实际不可用
            if self.network.is_offline(*nid) {
                continue;
            }
            if node.shard_leader(shard_id).is_some() {
                return Some(*nid);
            }
        }
        None
    }

    /// 向指定分片写入键值
    ///
    /// # Errors
    /// 无 Leader 或 propose 失败时返回。
    pub fn put(
        &mut self,
        shard_id: ShardId,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<(), RaftError> {
        let leader = self
            .shard_leader(shard_id)
            .ok_or(RaftError::ConfigError(format!(
                "no leader for shard {}",
                shard_id
            )))?;
        let node = self.nodes.get_mut(&leader).unwrap();
        node.propose(shard_id, ShardCommand::Put { key, value })?;
        Ok(())
    }

    /// 从指定分片删除键值
    ///
    /// # Errors
    /// 无 Leader 或 propose 失败时返回。
    pub fn delete(&mut self, shard_id: ShardId, key: Vec<u8>) -> Result<(), RaftError> {
        let leader = self
            .shard_leader(shard_id)
            .ok_or(RaftError::ConfigError(format!(
                "no leader for shard {}",
                shard_id
            )))?;
        let node = self.nodes.get_mut(&leader).unwrap();
        node.propose(shard_id, ShardCommand::Delete { key })?;
        Ok(())
    }

    /// 从指定分片 Leader 读取键值（强一致性读）
    ///
    /// 相比 `get`，此方法只从分片 Leader 读取，避免读到follower 的过期数据。
    pub fn get_from_leader(&self, shard_id: ShardId, key: &[u8]) -> Option<Vec<u8>> {
        let leader = self.shard_leader(shard_id)?;
        self.nodes
            .get(&leader)?
            .state_machine(shard_id)?
            .get(key)
            .map(|v| v.to_vec())
    }

    /// 从指定分片读取键值（从任意持有该分片的节点读）
    pub fn get(&self, shard_id: ShardId, key: &[u8]) -> Option<Vec<u8>> {
        for node in self.nodes.values() {
            if let Some(sm) = node.state_machine(shard_id) {
                if let Some(v) = sm.get(key) {
                    return Some(v.to_vec());
                }
            }
        }
        None
    }

    /// 跨分片范围扫描：仅从各分片 Leader 读取，保证强一致性
    ///
    /// **注意**：此方法按存储键的范围路由分片。Percolator 等使用键前缀编码
    ///（如 `0x03 || key`）的场景，前缀字节会改变排序，导致路由到错误分片。
    /// 此类场景请使用 [`ShardCluster::scan_shard`]，按原始键路由后扫描目标分片。
    pub fn scan_from_leaders(&self, range: &KeyRange) -> Vec<(Vec<u8>, Vec<u8>)> {
        let shard_ids = self.router.route_range(range);
        let mut results = Vec::new();
        for sid in shard_ids {
            if let Some(leader) = self.shard_leader(sid) {
                if let Some(node) = self.nodes.get(&leader) {
                    if let Some(sm) = node.state_machine(sid) {
                        results.extend(sm.scan(range));
                    }
                }
            }
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }

    /// 从指定分片 Leader 范围扫描（强一致性读）
    ///
    /// 用于 Percolator 等场景：按键前缀编码后的存储键可能与原始键的路由分片不一致，
    /// 因此由调用方先按原始键路由得到 `shard_id`，再扫描该分片内的范围。
    pub fn scan_shard(&self, shard_id: ShardId, range: &KeyRange) -> Vec<(Vec<u8>, Vec<u8>)> {
        let Some(leader) = self.shard_leader(shard_id) else {
            return Vec::new();
        };
        let Some(node) = self.nodes.get(&leader) else {
            return Vec::new();
        };
        let Some(sm) = node.state_machine(shard_id) else {
            return Vec::new();
        };
        sm.scan(range)
    }

    /// 跨分片范围扫描：聚合所有相关分片的结果
    pub fn scan(&self, range: &KeyRange) -> Vec<(Vec<u8>, Vec<u8>)> {
        let shard_ids = self.router.route_range(range);
        let mut results = Vec::new();
        for sid in shard_ids {
            for node in self.nodes.values() {
                if let Some(sm) = node.state_machine(sid) {
                    let partial = sm.scan(range);
                    results.extend(partial);
                }
            }
        }
        // 按键排序
        results.sort_by(|a, b| a.0.cmp(&b.0));
        // 去重（同键取最新，但由于只从 Leader 读，一般不会重复）
        results.dedup_by(|a, b| a.0 == b.0);
        results
    }

    /// 设置节点离线
    pub fn set_offline(&self, node: NodeId) {
        self.network.set_offline(node);
    }

    /// 设置节点上线
    pub fn set_online(&self, node: NodeId) {
        self.network.set_online(node);
    }

    // -----------------------------------------------------------------
    //  Phase 8.6：元数据 Raft 组管理
    // -----------------------------------------------------------------

    /// 查找元数据 Raft 组的 Leader 节点（跳过离线节点）
    pub fn meta_leader(&self) -> Option<NodeId> {
        for (&nid, meta_raft) in &self.meta_rafts {
            if self.network.is_offline(nid) {
                continue;
            }
            if meta_raft.state() == crate::raft::RaftState::Leader {
                return Some(nid);
            }
        }
        None
    }

    /// 向元数据 Raft 组提议一条元数据操作。
    ///
    /// 操作编码后通过元数据 Raft 组复制，所有节点 apply 后更新各自的元数据状态机。
    ///
    /// # Errors
    /// 无元数据 Leader 或 propose 失败时返回。
    pub fn propose_metadata(&mut self, op: MetadataOp) -> Result<(), RaftError> {
        let leader = self
            .meta_leader()
            .ok_or(RaftError::ConfigError("no meta leader".into()))?;
        let meta_raft = self
            .meta_rafts
            .get_mut(&leader)
            .expect("meta raft must exist");
        meta_raft.propose(op.encode())?;
        // 推进复制
        self.run_for(500);
        Ok(())
    }

    /// 从指定节点的元数据状态机重建路由器。
    ///
    /// 模拟节点崩溃后恢复：该节点的元数据状态机已从 Raft 日志重放所有操作，
    /// 调用此方法返回重建后的 ShardRouter。
    pub fn recover_router(&self, node_id: NodeId) -> Option<ShardRouter> {
        self.meta_sms.get(&node_id).map(|sm| sm.router().clone())
    }

    /// 获取指定节点的元数据状态机引用
    pub fn meta_state_machine(&self, node_id: NodeId) -> Option<&MetadataStateMachine> {
        self.meta_sms.get(&node_id)
    }
}

// =====================================================================
//  MultiRaftNode 的分片级 tick 扩展
// =====================================================================

impl MultiRaftNode {
    /// 推进单个分片的 Raft 时钟，返回产生的 RPC 消息
    pub fn tick_shard(&mut self, shard_id: ShardId, elapsed_ms: u64) -> Vec<RpcMessage> {
        let raft = match self.raft_groups.get_mut(&shard_id) {
            Some(r) => r,
            None => return Vec::new(),
        };
        let msgs = raft.tick(elapsed_ms);
        // 推进 apply
        let applied = raft.apply();
        let sm = self
            .state_machines
            .get_mut(&shard_id)
            .expect("state machine must exist");
        for entry in applied {
            sm.apply_raw(&entry.command);
        }
        msgs
    }

    /// 获取分片的 Raft 节点可变引用（用于成员变更等操作）
    pub fn raft_group_mut(&mut self, shard_id: ShardId) -> Option<&mut RaftNode> {
        self.raft_groups.get_mut(&shard_id)
    }

    /// 离开分片（移除本节点上的分片 Raft 组和状态机）
    pub fn leave_shard(&mut self, shard_id: ShardId) {
        self.raft_groups.remove(&shard_id);
        self.state_machines.remove(&shard_id);
    }
}

// =====================================================================
//  测试模块
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  1. KeyRange 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_key_range_unbounded_contains_all() {
        let r = KeyRange::unbounded();
        assert!(r.contains(b""));
        assert!(r.contains(b"abc"));
        assert!(r.contains(&[0xFF]));
    }

    #[test]
    fn test_key_range_new_contains() {
        let r = KeyRange::new(b"a".to_vec(), b"z".to_vec());
        assert!(r.contains(b"a"));
        assert!(r.contains(b"m"));
        assert!(!r.contains(b"z"));
        assert!(!r.contains(b"A"));
    }

    #[test]
    fn test_key_range_from_contains() {
        let r = KeyRange::from(b"m".to_vec());
        assert!(!r.contains(b"a"));
        assert!(r.contains(b"m"));
        assert!(r.contains(b"z"));
    }

    #[test]
    fn test_key_range_overlaps() {
        let r1 = KeyRange::new(b"a".to_vec(), b"m".to_vec());
        let r2 = KeyRange::new(b"l".to_vec(), b"z".to_vec());
        assert!(r1.overlaps(&r2));

        // r1=[a,m), r3=[a,l) → r3 是 r1 的子集，重叠
        let r3 = KeyRange::new(b"a".to_vec(), b"l".to_vec());
        assert!(r1.overlaps(&r3));

        // r1=[a,m), r4=[m,z) → r1.end=m, r4.start=m → 边界相邻不重叠
        let r4 = KeyRange::new(b"m".to_vec(), b"z".to_vec());
        assert!(!r1.overlaps(&r4));
    }

    #[test]
    fn test_key_range_unbounded_overlaps_all() {
        let r1 = KeyRange::unbounded();
        let r2 = KeyRange::new(b"a".to_vec(), b"z".to_vec());
        assert!(r1.overlaps(&r2));
        assert!(r2.overlaps(&r1));
    }

    // -----------------------------------------------------------------
    //  2. ShardCommand 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn test_command_put_encode_decode() {
        let cmd = ShardCommand::Put {
            key: b"hello".to_vec(),
            value: b"world".to_vec(),
        };
        let encoded = cmd.encode();
        let decoded = ShardCommand::decode(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn test_command_delete_encode_decode() {
        let cmd = ShardCommand::Delete {
            key: b"key1".to_vec(),
        };
        let encoded = cmd.encode();
        let decoded = ShardCommand::decode(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn test_command_put_empty_value() {
        let cmd = ShardCommand::Put {
            key: b"k".to_vec(),
            value: vec![],
        };
        let encoded = cmd.encode();
        let decoded = ShardCommand::decode(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn test_command_put_empty_key() {
        let cmd = ShardCommand::Put {
            key: vec![],
            value: b"v".to_vec(),
        };
        let encoded = cmd.encode();
        let decoded = ShardCommand::decode(&encoded).unwrap();
        assert_eq!(cmd, decoded);
    }

    #[test]
    fn test_command_decode_empty_data() {
        let result = ShardCommand::decode(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_decode_invalid_cmd_type() {
        let data = [0xFF];
        let result = ShardCommand::decode(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_decode_truncated_key_length() {
        let data = [0x01, 0x00, 0x00]; // 不足 4 字节
        let result = ShardCommand::decode(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_decode_truncated_key() {
        // key_len=5 但只有 2 字节 key
        let data = [0x01, 0x00, 0x00, 0x00, 0x05, b'a', b'b'];
        let result = ShardCommand::decode(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_decode_truncated_value_length() {
        // Put with key="ab" but missing value length
        let mut data = vec![0x01];
        data.extend_from_slice(&2u32.to_be_bytes());
        data.extend_from_slice(b"ab");
        // 缺少 value_len 的 4 字节
        let result = ShardCommand::decode(&data);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    //  3. KvStateMachine 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_kv_state_machine_put_get() {
        let mut sm = KvStateMachine::new();
        sm.apply_command(&ShardCommand::Put {
            key: b"k1".to_vec(),
            value: b"v1".to_vec(),
        });
        assert_eq!(sm.get(b"k1"), Some("v1".as_bytes()));
    }

    #[test]
    fn test_kv_state_machine_delete() {
        let mut sm = KvStateMachine::new();
        sm.apply_command(&ShardCommand::Put {
            key: b"k1".to_vec(),
            value: b"v1".to_vec(),
        });
        sm.apply_command(&ShardCommand::Delete {
            key: b"k1".to_vec(),
        });
        assert_eq!(sm.get(b"k1"), None);
    }

    #[test]
    fn test_kv_state_machine_overwrite() {
        let mut sm = KvStateMachine::new();
        sm.apply_command(&ShardCommand::Put {
            key: b"k1".to_vec(),
            value: b"v1".to_vec(),
        });
        sm.apply_command(&ShardCommand::Put {
            key: b"k1".to_vec(),
            value: b"v2".to_vec(),
        });
        assert_eq!(sm.get(b"k1"), Some("v2".as_bytes()));
    }

    #[test]
    fn test_kv_state_machine_scan() {
        let mut sm = KvStateMachine::new();
        for i in 0..10u8 {
            sm.apply_command(&ShardCommand::Put {
                key: vec![i],
                value: vec![i * 2],
            });
        }
        // 扫描 [3, 7)
        let range = KeyRange::new(vec![3], vec![7]);
        let results = sm.scan(&range);
        assert_eq!(results.len(), 4);
        assert_eq!(results[0], (vec![3], vec![6]));
        assert_eq!(results[3], (vec![6], vec![12]));
    }

    #[test]
    fn test_kv_state_machine_scan_unbounded() {
        let mut sm = KvStateMachine::new();
        sm.apply_command(&ShardCommand::Put {
            key: b"a".to_vec(),
            value: b"1".to_vec(),
        });
        sm.apply_command(&ShardCommand::Put {
            key: b"b".to_vec(),
            value: b"2".to_vec(),
        });
        let results = sm.scan(&KeyRange::unbounded());
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_kv_state_machine_apply_raw() {
        let mut sm = KvStateMachine::new();
        let cmd = ShardCommand::Put {
            key: b"k".to_vec(),
            value: b"v".to_vec(),
        };
        sm.apply_raw(&cmd.encode());
        assert_eq!(sm.get(b"k"), Some("v".as_bytes()));
    }

    #[test]
    fn test_kv_state_machine_len_is_empty() {
        let sm = KvStateMachine::new();
        assert!(sm.is_empty());
        assert_eq!(sm.len(), 0);
    }

    // -----------------------------------------------------------------
    //  4. ShardRouter 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_router_route_single_shard() {
        let mut router = ShardRouter::new();
        router.add_shard(Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]));
        assert_eq!(router.route(b"any_key").unwrap(), 1);
    }

    #[test]
    fn test_router_route_two_shards() {
        let mut router = ShardRouter::new();
        router.add_shard(Shard::new(
            1,
            KeyRange::new(b"a".to_vec(), b"m".to_vec()),
            vec![1, 2],
        ));
        router.add_shard(Shard::new(
            2,
            KeyRange::new(b"m".to_vec(), b"z".to_vec()),
            vec![2, 3],
        ));
        assert_eq!(router.route(b"a").unwrap(), 1);
        assert_eq!(router.route(b"l").unwrap(), 1);
        assert_eq!(router.route(b"m").unwrap(), 2);
        assert_eq!(router.route(b"y").unwrap(), 2);
    }

    #[test]
    fn test_router_route_no_coverage() {
        let router = ShardRouter::new();
        assert!(router.route(b"key").is_err());
    }

    #[test]
    fn test_router_route_range_overlapping() {
        let mut router = ShardRouter::new();
        router.add_shard(Shard::new(
            1,
            KeyRange::new(b"a".to_vec(), b"m".to_vec()),
            vec![1],
        ));
        router.add_shard(Shard::new(
            2,
            KeyRange::new(b"m".to_vec(), b"z".to_vec()),
            vec![2],
        ));
        router.add_shard(Shard::new(
            3,
            KeyRange::new(b"z".to_vec(), b"~".to_vec()),
            vec![3],
        ));

        // 查询 [l, x) → 覆盖 shard 1 [a,m) (部分 l-m) + shard 2 [m,z) (全部 m-x)
        // shard 3 [z,~) 不重叠（z > x）
        let range = KeyRange::new(b"l".to_vec(), b"x".to_vec());
        let ids = router.route_range(&range);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_router_remove_shard() {
        let mut router = ShardRouter::new();
        router.add_shard(Shard::new(1, KeyRange::unbounded(), vec![1]));
        assert!(router.remove_shard(1).is_some());
        assert!(router.route(b"key").is_err());
    }

    #[test]
    fn test_router_get_shard() {
        let mut router = ShardRouter::new();
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2]);
        router.add_shard(shard.clone());
        let got = router.get_shard(1).unwrap();
        assert_eq!(got.id, 1);
        assert_eq!(got.peers, vec![1, 2]);
    }

    // -----------------------------------------------------------------
    //  5. 端到端集成：3 节点各管理 1-2 个分片
    // -----------------------------------------------------------------

    #[test]
    fn test_shard_cluster_single_shard_3_nodes() {
        // 3 节点、1 个无界分片 → 验证基本写入和读取
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 6000);

        // 等待选举
        cluster.run_for(1000);
        let leader = cluster.shard_leader(1).expect("leader should exist");

        // 写入数据
        for i in 0..10u8 {
            cluster.put(1, vec![i], vec![i * 2]).unwrap();
            cluster.run_for(100);
        }
        cluster.run_for(500);

        // 验证所有节点都能读到数据
        for i in 0..10u8 {
            let val = cluster.get(1, &[i]).expect("value should exist");
            assert_eq!(val, vec![i * 2]);
        }

        // 验证 Leader 状态机有 10 条数据
        let sm = cluster
            .nodes
            .get(&leader)
            .unwrap()
            .state_machine(1)
            .unwrap();
        assert_eq!(sm.len(), 10);
    }

    #[test]
    fn test_shard_cluster_two_shards_route_correctly() {
        // 3 节点、2 个分片
        // shard 1: [a, m) → nodes 1, 2
        // shard 2: [m, ~) → nodes 2, 3
        let shard1 = Shard::new(1, KeyRange::new(b"a".to_vec(), b"m".to_vec()), vec![1, 2]);
        let shard2 = Shard::new(2, KeyRange::new(b"m".to_vec(), b"~".to_vec()), vec![2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard1, shard2], 6100);

        cluster.run_for(1000);

        // 写入 shard 1 的数据
        cluster.put(1, b"apple".to_vec(), b"red".to_vec()).unwrap();
        cluster
            .put(1, b"banana".to_vec(), b"yellow".to_vec())
            .unwrap();
        cluster.run_for(300);

        // 写入 shard 2 的数据
        cluster
            .put(2, b"orange".to_vec(), b"orange".to_vec())
            .unwrap();
        cluster.put(2, b"pear".to_vec(), b"green".to_vec()).unwrap();
        cluster.run_for(300);

        // 验证路由正确
        assert_eq!(cluster.get(1, b"apple"), Some(b"red".to_vec()));
        assert_eq!(cluster.get(1, b"banana"), Some(b"yellow".to_vec()));
        assert_eq!(cluster.get(2, b"orange"), Some(b"orange".to_vec()));
        assert_eq!(cluster.get(2, b"pear"), Some(b"green".to_vec()));

        // 验证跨分片不存在
        assert_eq!(cluster.get(1, b"orange"), None);
        assert_eq!(cluster.get(2, b"apple"), None);
    }

    #[test]
    fn test_shard_cluster_cross_shard_scan() {
        // 3 节点、2 个分片 → 跨分片扫描聚合
        let shard1 = Shard::new(
            1,
            KeyRange::new(b"a".to_vec(), b"m".to_vec()),
            vec![1, 2, 3],
        );
        let shard2 = Shard::new(
            2,
            KeyRange::new(b"m".to_vec(), b"~".to_vec()),
            vec![1, 2, 3],
        );
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard1, shard2], 6200);

        cluster.run_for(1000);

        // 写入 shard 1: a, b, c, ..., l
        for ch in b'a'..=b'l' {
            cluster.put(1, vec![ch], vec![ch - b'a' + 1]).unwrap();
        }
        cluster.run_for(300);

        // 写入 shard 2: m, n, o, ..., z
        for ch in b'm'..=b'z' {
            cluster.put(2, vec![ch], vec![ch - b'a' + 1]).unwrap();
        }
        cluster.run_for(500);

        // 跨分片扫描 [d, p) → 应包含 d..=o（shard1 的 d-l + shard2 的 m-o）
        let range = KeyRange::new(b"d".to_vec(), b"p".to_vec());
        let results = cluster.scan(&range);
        assert_eq!(results.len(), 12); // d, e, f, g, h, i, j, k, l, m, n, o

        // 验证有序
        for i in 1..results.len() {
            assert!(results[i - 1].0 < results[i].0, "results should be sorted");
        }

        // 验证首尾
        assert_eq!(results[0].0, b"d");
        assert_eq!(results[11].0, b"o");
    }

    #[test]
    fn test_shard_cluster_3_nodes_2_shards_each() {
        // 3 节点、4 个分片 → 每个节点管理 2-3 个分片
        // shard 1: [a, g) → nodes 1, 2
        // shard 2: [g, m) → nodes 2, 3
        // shard 3: [m, s) → nodes 1, 3
        // shard 4: [s, ~) → nodes 1, 2, 3
        let shards = vec![
            Shard::new(1, KeyRange::new(b"a".to_vec(), b"g".to_vec()), vec![1, 2]),
            Shard::new(2, KeyRange::new(b"g".to_vec(), b"m".to_vec()), vec![2, 3]),
            Shard::new(3, KeyRange::new(b"m".to_vec(), b"s".to_vec()), vec![1, 3]),
            Shard::new(
                4,
                KeyRange::new(b"s".to_vec(), b"~".to_vec()),
                vec![1, 2, 3],
            ),
        ];
        let mut cluster = ShardCluster::new(&[1, 2, 3], shards, 6300);

        cluster.run_for(1000);

        // 每个分片写入数据
        cluster.put(1, b"abc".to_vec(), b"v1".to_vec()).unwrap();
        cluster.put(2, b"ghi".to_vec(), b"v2".to_vec()).unwrap();
        cluster.put(3, b"nop".to_vec(), b"v3".to_vec()).unwrap();
        cluster.put(4, b"xyz".to_vec(), b"v4".to_vec()).unwrap();
        cluster.run_for(500);

        // 验证路由
        assert_eq!(cluster.get(1, b"abc"), Some(b"v1".to_vec()));
        assert_eq!(cluster.get(2, b"ghi"), Some(b"v2".to_vec()));
        assert_eq!(cluster.get(3, b"nop"), Some(b"v3".to_vec()));
        assert_eq!(cluster.get(4, b"xyz"), Some(b"v4".to_vec()));

        // 验证跨分片全表扫描
        let all = cluster.scan(&KeyRange::unbounded());
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn test_shard_cluster_node_2_offline_shard1_survives() {
        // shard 1: nodes 1, 2 → node 2 离线，node 1 仍可用
        let shard1 = Shard::new(1, KeyRange::unbounded(), vec![1, 2]);
        let mut cluster = ShardCluster::new(&[1, 2], vec![shard1], 6400);

        cluster.run_for(1000);
        cluster.put(1, b"k1".to_vec(), b"v1".to_vec()).unwrap();
        cluster.run_for(300);

        // node 2 离线
        cluster.set_offline(2);
        cluster.run_for(1000);

        // node 1 仍可读写
        let leader = cluster.shard_leader(1).expect("leader should exist");
        assert_eq!(leader, 1);
        assert_eq!(cluster.get(1, b"k1"), Some(b"v1".to_vec()));
    }

    #[test]
    fn test_shard_cluster_write_after_leader_failover() {
        // 3 节点单分片 → kill leader → 新 leader 选举 → 继续写入
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 6500);

        cluster.run_for(1000);
        cluster.put(1, b"init".to_vec(), b"v0".to_vec()).unwrap();
        cluster.run_for(300);

        let leader = cluster.shard_leader(1).unwrap();
        cluster.set_offline(leader);
        cluster.run_for(1000);

        // 新 Leader 应能继续写入
        let new_leader = cluster.shard_leader(1).expect("new leader should exist");
        assert_ne!(new_leader, leader);
        cluster
            .put(1, b"after_failover".to_vec(), b"v1".to_vec())
            .unwrap();
        cluster.run_for(300);

        // 恢复原 Leader
        cluster.set_online(leader);
        cluster.run_for(500);

        // 验证数据
        assert_eq!(cluster.get(1, b"init"), Some(b"v0".to_vec()));
        assert_eq!(cluster.get(1, b"after_failover"), Some(b"v1".to_vec()));
    }

    #[test]
    fn test_shard_cluster_concurrent_writes_different_shards() {
        // 2 个分片，并发写入不同分片，互不干扰
        let shard1 = Shard::new(
            1,
            KeyRange::new(b"a".to_vec(), b"m".to_vec()),
            vec![1, 2, 3],
        );
        let shard2 = Shard::new(
            2,
            KeyRange::new(b"m".to_vec(), b"~".to_vec()),
            vec![1, 2, 3],
        );
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard1, shard2], 6600);

        cluster.run_for(1000);

        // 交替写入两个分片
        for i in 0..20u8 {
            let key1 = vec![b'a' + (i % 12)];
            let key2 = vec![b'm' + (i % 13)];
            cluster.put(1, key1, vec![i]).unwrap();
            cluster.put(2, key2, vec![i]).unwrap();
            cluster.run_for(50);
        }
        cluster.run_for(500);

        // 两个分片都有数据
        let sm1_count: usize = cluster
            .nodes
            .values()
            .map(|n| n.state_machine(1).map_or(0, |sm| sm.len()))
            .max()
            .unwrap();
        let sm2_count: usize = cluster
            .nodes
            .values()
            .map(|n| n.state_machine(2).map_or(0, |sm| sm.len()))
            .max()
            .unwrap();
        assert!(sm1_count > 0, "shard 1 should have data");
        assert!(sm2_count > 0, "shard 2 should have data");
    }

    #[test]
    fn test_shard_cluster_delete_across_shards() {
        // 跨分片删除验证
        let shard1 = Shard::new(
            1,
            KeyRange::new(b"a".to_vec(), b"m".to_vec()),
            vec![1, 2, 3],
        );
        let shard2 = Shard::new(
            2,
            KeyRange::new(b"m".to_vec(), b"~".to_vec()),
            vec![1, 2, 3],
        );
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard1, shard2], 6700);

        cluster.run_for(1000);

        // 写入
        cluster.put(1, b"key1".to_vec(), b"val1".to_vec()).unwrap();
        cluster.put(2, b"key2".to_vec(), b"val2".to_vec()).unwrap();
        cluster.run_for(300);

        assert_eq!(cluster.get(1, b"key1"), Some(b"val1".to_vec()));
        assert_eq!(cluster.get(2, b"key2"), Some(b"val2".to_vec()));

        // 删除
        let leader1 = cluster.shard_leader(1).unwrap();
        let leader2 = cluster.shard_leader(2).unwrap();
        cluster
            .nodes
            .get_mut(&leader1)
            .unwrap()
            .propose(
                1,
                ShardCommand::Delete {
                    key: b"key1".to_vec(),
                },
            )
            .unwrap();
        cluster
            .nodes
            .get_mut(&leader2)
            .unwrap()
            .propose(
                2,
                ShardCommand::Delete {
                    key: b"key2".to_vec(),
                },
            )
            .unwrap();
        cluster.run_for(500);

        // 验证删除
        assert_eq!(cluster.get(1, b"key1"), None);
        assert_eq!(cluster.get(2, b"key2"), None);
    }

    #[test]
    fn test_shard_cluster_scan_empty_range() {
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 6800);

        cluster.run_for(1000);
        cluster.put(1, b"k1".to_vec(), b"v1".to_vec()).unwrap();
        cluster.run_for(300);

        // 扫描一个空范围 [z, ~)
        let range = KeyRange::new(b"z".to_vec(), b"~".to_vec());
        let results = cluster.scan(&range);
        assert!(results.is_empty());
    }

    #[test]
    fn test_shard_cluster_100_keys_across_2_shards() {
        // 100 个键分布在 2 个分片，验证数据完整性
        let shard1 = Shard::new(1, KeyRange::new(b"".to_vec(), b"M".to_vec()), vec![1, 2, 3]);
        let shard2 = Shard::new(
            2,
            KeyRange::new(b"M".to_vec(), b"~".to_vec()),
            vec![1, 2, 3],
        );
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard1, shard2], 6900);

        cluster.run_for(1000);

        // 写入 100 个键
        for i in 0..100u8 {
            let key = format!("key{:03}", i).into_bytes();
            let value = vec![i];
            // 路由到正确的分片
            let shard_id = cluster.router.route(&key).unwrap();
            cluster.put(shard_id, key, value).unwrap();
        }
        cluster.run_for(1000);

        // 验证全部可读
        for i in 0..100u8 {
            let key = format!("key{:03}", i).into_bytes();
            let expected = vec![i];
            // 找到正确的分片
            let shard_id = cluster.router.route(&key).unwrap();
            let val = cluster.get(shard_id, &key).expect("value should exist");
            assert_eq!(val, expected, "key {} value mismatch", i);
        }

        // 跨分片全表扫描
        let all = cluster.scan(&KeyRange::unbounded());
        assert_eq!(all.len(), 100);
    }

    // -----------------------------------------------------------------
    //  6. 错误处理测试
    // -----------------------------------------------------------------

    #[test]
    fn test_propose_to_nonexistent_shard() {
        let router = ShardRouter::new();
        let mut node = MultiRaftNode::new(1, router);
        let result = node.propose(
            999,
            ShardCommand::Put {
                key: b"k".to_vec(),
                value: b"v".to_vec(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_propose_on_non_leader() {
        // node 2 不是 Leader（单节点不分片）
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1]);
        let mut router = ShardRouter::new();
        router.add_shard(shard.clone());
        let mut node = MultiRaftNode::new(2, router);
        // node 2 不在 peers 中，但仍然 join（虽然不会成为 Leader）
        node.join_shard(&shard, 7000);
        let result = node.propose(
            1,
            ShardCommand::Put {
                key: b"k".to_vec(),
                value: b"v".to_vec(),
            },
        );
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    //  7. ShardedMessage / ShardNetwork 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_shard_network_offline_drops_messages() {
        let mut net = ShardNetwork::new();
        net.set_offline(2);
        let req = crate::raft::RequestVoteRequest {
            term: 1,
            candidate_id: 1,
            last_log_index: 0,
            last_log_term: 0,
        };
        let msg = RpcMessage::new(1, 2, MessageType::RequestVoteRequest(req));
        net.send(1, msg);
        assert!(net.drain().is_empty());
    }

    #[test]
    fn test_shard_network_partition_drops_messages() {
        let mut net = ShardNetwork::new();
        net.partition(1, 2);
        let req = crate::raft::RequestVoteRequest {
            term: 1,
            candidate_id: 1,
            last_log_index: 0,
            last_log_term: 0,
        };
        let msg = RpcMessage::new(1, 2, MessageType::RequestVoteRequest(req));
        net.send(1, msg);
        assert!(net.drain().is_empty());
    }

    #[test]
    fn test_shard_network_heal_restores_messages() {
        let mut net = ShardNetwork::new();
        net.partition(1, 2);
        net.heal_all();
        let req = crate::raft::RequestVoteRequest {
            term: 1,
            candidate_id: 1,
            last_log_index: 0,
            last_log_term: 0,
        };
        let msg = RpcMessage::new(1, 2, MessageType::RequestVoteRequest(req));
        net.send(1, msg);
        assert_eq!(net.drain().len(), 1);
    }

    // -----------------------------------------------------------------
    //  8. Phase 8.5：分片分裂测试
    // -----------------------------------------------------------------

    #[test]
    fn test_split_shard_basic() {
        // 3 节点、1 个无界分片 → 写入数据 → 在 "m" 处分裂为两个分片
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 7000);

        cluster.run_for(1000);

        // 写入跨分裂键的数据
        cluster.put(1, b"apple".to_vec(), b"red".to_vec()).unwrap();
        cluster
            .put(1, b"banana".to_vec(), b"yellow".to_vec())
            .unwrap();
        cluster
            .put(1, b"orange".to_vec(), b"orange".to_vec())
            .unwrap();
        cluster.put(1, b"pear".to_vec(), b"green".to_vec()).unwrap();
        cluster.run_for(500);

        // 分裂
        let (new_id_1, new_id_2) = cluster.split_shard(1, b"m".to_vec(), 7100).unwrap();

        // 验证路由：apple/banana 在新分片1，orange/pear 在新分片2
        assert_eq!(cluster.router.route(b"apple").unwrap(), new_id_1);
        assert_eq!(cluster.router.route(b"banana").unwrap(), new_id_1);
        assert_eq!(cluster.router.route(b"orange").unwrap(), new_id_2);
        assert_eq!(cluster.router.route(b"pear").unwrap(), new_id_2);

        // 验证数据完整
        assert_eq!(cluster.get(new_id_1, b"apple"), Some(b"red".to_vec()));
        assert_eq!(cluster.get(new_id_1, b"banana"), Some(b"yellow".to_vec()));
        assert_eq!(cluster.get(new_id_2, b"orange"), Some(b"orange".to_vec()));
        assert_eq!(cluster.get(new_id_2, b"pear"), Some(b"green".to_vec()));
    }

    #[test]
    fn test_split_shard_data_redistribution() {
        // 写入大量数据后分裂，验证全部数据正确重分布
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 7200);

        cluster.run_for(1000);

        // 写入 a-z 共 26 个键
        for ch in b'a'..=b'z' {
            cluster.put(1, vec![ch], vec![ch]).unwrap();
        }
        cluster.run_for(1000);

        // 在 'm' 处分裂：[a, m) 和 [m, ~)
        let (id_left, id_right) = cluster.split_shard(1, b"m".to_vec(), 7300).unwrap();

        // 验证左半部分 a-l（12 个键）
        for ch in b'a'..=b'l' {
            assert_eq!(
                cluster.get(id_left, &[ch]),
                Some(vec![ch]),
                "key {} should be in left shard",
                ch as char
            );
        }

        // 验证右半部分 m-z（14 个键）
        for ch in b'm'..=b'z' {
            assert_eq!(
                cluster.get(id_right, &[ch]),
                Some(vec![ch]),
                "key {} should be in right shard",
                ch as char
            );
        }

        // 跨分片全表扫描验证总数
        let all = cluster.scan(&KeyRange::unbounded());
        assert_eq!(all.len(), 26);
    }

    #[test]
    fn test_split_shard_write_after_split() {
        // 分裂后仍可向新分片写入数据
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 7400);

        cluster.run_for(1000);
        cluster.put(1, b"before".to_vec(), b"v0".to_vec()).unwrap();
        cluster.run_for(300);

        let (id_left, id_right) = cluster.split_shard(1, b"m".to_vec(), 7500).unwrap();

        // 分裂后写入新数据
        cluster
            .put(id_left, b"after_left".to_vec(), b"vL".to_vec())
            .unwrap();
        cluster
            .put(id_right, b"after_right".to_vec(), b"vR".to_vec())
            .unwrap();
        cluster.run_for(500);

        // 验证旧数据和新数据都在
        assert_eq!(cluster.get(id_left, b"before"), Some(b"v0".to_vec()));
        assert_eq!(cluster.get(id_left, b"after_left"), Some(b"vL".to_vec()));
        assert_eq!(cluster.get(id_right, b"after_right"), Some(b"vR".to_vec()));
    }

    #[test]
    fn test_split_shard_empty_shard() {
        // 空分片分裂 → 两个空分片
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 7600);

        cluster.run_for(1000);

        let (id_left, id_right) = cluster.split_shard(1, b"m".to_vec(), 7700).unwrap();

        // 验证两个新分片都有 Leader
        assert!(cluster.shard_leader(id_left).is_some());
        assert!(cluster.shard_leader(id_right).is_some());

        // 验证空数据
        let all = cluster.scan(&KeyRange::unbounded());
        assert!(all.is_empty());
    }

    #[test]
    fn test_split_shard_nonexistent_shard() {
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 7800);

        cluster.run_for(1000);

        // 分裂不存在的分片 → 报错
        let result = cluster.split_shard(999, b"m".to_vec(), 7900);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    //  9. Phase 8.5：分片迁移测试
    // -----------------------------------------------------------------

    #[test]
    fn test_migrate_shard_basic() {
        // 4 节点，shard 1 初始在 [1,2,3]，迁移到 [1,2,4]
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3, 4], vec![shard], 8000);

        cluster.run_for(1000);

        // 写入数据
        cluster.put(1, b"k1".to_vec(), b"v1".to_vec()).unwrap();
        cluster.put(1, b"k2".to_vec(), b"v2".to_vec()).unwrap();
        cluster.run_for(500);

        // 迁移：[1,2,3] → [1,2,4]（移除 3，添加 4）
        cluster.migrate_shard(1, vec![1, 2, 4], 8100).unwrap();

        // 验证数据完整
        assert_eq!(cluster.get(1, b"k1"), Some(b"v1".to_vec()));
        assert_eq!(cluster.get(1, b"k2"), Some(b"v2".to_vec()));

        // 验证路由器的 peers 已更新
        let updated_shard = cluster.router.get_shard(1).unwrap();
        assert_eq!(updated_shard.peers, vec![1, 2, 4]);
    }

    #[test]
    fn test_migrate_shard_write_after_migrate() {
        // 迁移后仍可写入
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3, 4, 5], vec![shard], 8200);

        cluster.run_for(1000);
        cluster.put(1, b"before".to_vec(), b"v0".to_vec()).unwrap();
        cluster.run_for(300);

        // 迁移到全新节点集 [4, 5, 1]
        cluster.migrate_shard(1, vec![4, 5, 1], 8300).unwrap();

        // 迁移后写入
        cluster.put(1, b"after".to_vec(), b"v1".to_vec()).unwrap();
        cluster.run_for(500);

        // 验证旧数据和新数据
        assert_eq!(cluster.get(1, b"before"), Some(b"v0".to_vec()));
        assert_eq!(cluster.get(1, b"after"), Some(b"v1".to_vec()));
    }

    #[test]
    fn test_migrate_shard_no_membership_change() {
        // 迁移到相同 peers → 无操作
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 8400);

        cluster.run_for(1000);
        cluster.put(1, b"k1".to_vec(), b"v1".to_vec()).unwrap();
        cluster.run_for(300);

        // 迁移到相同配置 → 无操作
        cluster.migrate_shard(1, vec![1, 2, 3], 8500).unwrap();

        // 数据仍可读
        assert_eq!(cluster.get(1, b"k1"), Some(b"v1".to_vec()));
    }

    #[test]
    fn test_migrate_shard_nonexistent_shard() {
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 8600);

        cluster.run_for(1000);

        let result = cluster.migrate_shard(999, vec![1, 2], 8700);
        assert!(result.is_err());
    }

    #[test]
    fn test_migrate_then_split() {
        // 先迁移再分裂：验证组合操作
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3, 4], vec![shard], 8800);

        cluster.run_for(1000);

        // 写入数据
        for ch in b'a'..=b'z' {
            cluster.put(1, vec![ch], vec![ch]).unwrap();
        }
        cluster.run_for(1000);

        // 迁移到 [1, 2, 4]
        cluster.migrate_shard(1, vec![1, 2, 4], 8900).unwrap();

        // 迁移后分裂
        let (id_left, id_right) = cluster.split_shard(1, b"m".to_vec(), 9000).unwrap();

        // 验证数据完整
        for ch in b'a'..=b'l' {
            assert_eq!(cluster.get(id_left, &[ch]), Some(vec![ch]));
        }
        for ch in b'm'..=b'z' {
            assert_eq!(cluster.get(id_right, &[ch]), Some(vec![ch]));
        }
    }

    // -----------------------------------------------------------------
    //  10. Phase 8.6：MetadataOp 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn test_metadata_op_add_shard_encode_decode() {
        let op = MetadataOp::AddShard(Shard::new(
            5,
            KeyRange::new(b"a".to_vec(), b"z".to_vec()),
            vec![1, 2, 3],
        ));
        let encoded = op.encode();
        let decoded = MetadataOp::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn test_metadata_op_add_shard_unbounded_range() {
        let op = MetadataOp::AddShard(Shard::new(1, KeyRange::unbounded(), vec![1]));
        let encoded = op.encode();
        let decoded = MetadataOp::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn test_metadata_op_remove_shard_encode_decode() {
        let op = MetadataOp::RemoveShard(42);
        let encoded = op.encode();
        let decoded = MetadataOp::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn test_metadata_op_update_peers_encode_decode() {
        let op = MetadataOp::UpdatePeers {
            shard_id: 3,
            new_peers: vec![4, 5, 6],
        };
        let encoded = op.encode();
        let decoded = MetadataOp::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn test_metadata_op_split_shard_encode_decode() {
        let op = MetadataOp::SplitShard {
            old_id: 1,
            new_id_1: 2,
            new_id_2: 3,
            split_key: b"m".to_vec(),
            original_range: KeyRange::unbounded(),
            peers: vec![1, 2, 3],
        };
        let encoded = op.encode();
        let decoded = MetadataOp::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn test_metadata_op_decode_empty() {
        assert!(MetadataOp::decode(&[]).is_err());
    }

    #[test]
    fn test_metadata_op_decode_invalid_type() {
        assert!(MetadataOp::decode(&[0xFF]).is_err());
    }

    // -----------------------------------------------------------------
    //  11. Phase 8.6：MetadataStateMachine 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_metadata_state_machine_add_shard() {
        let mut sm = MetadataStateMachine::new();
        assert_eq!(sm.shard_count(), 0);
        sm.apply(&MetadataOp::AddShard(Shard::new(
            1,
            KeyRange::unbounded(),
            vec![1, 2, 3],
        )));
        assert_eq!(sm.shard_count(), 1);
        assert_eq!(sm.router().route(b"any").unwrap(), 1);
    }

    #[test]
    fn test_metadata_state_machine_remove_shard() {
        let mut sm = MetadataStateMachine::new();
        sm.apply(&MetadataOp::AddShard(Shard::new(
            1,
            KeyRange::unbounded(),
            vec![1],
        )));
        sm.apply(&MetadataOp::RemoveShard(1));
        assert_eq!(sm.shard_count(), 0);
        assert!(sm.router().route(b"key").is_err());
    }

    #[test]
    fn test_metadata_state_machine_update_peers() {
        let mut sm = MetadataStateMachine::new();
        sm.apply(&MetadataOp::AddShard(Shard::new(
            1,
            KeyRange::unbounded(),
            vec![1, 2, 3],
        )));
        sm.apply(&MetadataOp::UpdatePeers {
            shard_id: 1,
            new_peers: vec![4, 5],
        });
        let shard = sm.router().get_shard(1).unwrap();
        assert_eq!(shard.peers, vec![4, 5]);
    }

    #[test]
    fn test_metadata_state_machine_split_shard() {
        let mut sm = MetadataStateMachine::new();
        sm.apply(&MetadataOp::AddShard(Shard::new(
            1,
            KeyRange::unbounded(),
            vec![1, 2, 3],
        )));
        sm.apply(&MetadataOp::SplitShard {
            old_id: 1,
            new_id_1: 2,
            new_id_2: 3,
            split_key: b"m".to_vec(),
            original_range: KeyRange::unbounded(),
            peers: vec![1, 2, 3],
        });
        assert_eq!(sm.shard_count(), 2);
        // 旧分片已移除
        assert!(sm.router().get_shard(1).is_none());
        // 新分片路由正确
        assert_eq!(sm.router().route(b"a").unwrap(), 2);
        assert_eq!(sm.router().route(b"m").unwrap(), 3);
        assert_eq!(sm.router().route(b"z").unwrap(), 3);
    }

    #[test]
    fn test_metadata_state_machine_apply_raw() {
        let mut sm = MetadataStateMachine::new();
        let op = MetadataOp::AddShard(Shard::new(1, KeyRange::unbounded(), vec![1]));
        sm.apply_raw(&op.encode());
        assert_eq!(sm.shard_count(), 1);
    }

    #[test]
    fn test_metadata_state_machine_replay_full_history() {
        // 模拟从日志重放完整历史：AddShard → SplitShard → UpdatePeers
        let mut sm = MetadataStateMachine::new();

        // 1. 添加分片
        sm.apply(&MetadataOp::AddShard(Shard::new(
            1,
            KeyRange::unbounded(),
            vec![1, 2, 3],
        )));

        // 2. 分裂
        sm.apply(&MetadataOp::SplitShard {
            old_id: 1,
            new_id_1: 2,
            new_id_2: 3,
            split_key: b"m".to_vec(),
            original_range: KeyRange::unbounded(),
            peers: vec![1, 2, 3],
        });

        // 3. 迁移分片 2
        sm.apply(&MetadataOp::UpdatePeers {
            shard_id: 2,
            new_peers: vec![4, 5],
        });

        // 验证重建的路由器
        assert_eq!(sm.shard_count(), 2);
        assert!(sm.router().get_shard(1).is_none());
        assert_eq!(sm.router().route(b"a").unwrap(), 2);
        assert_eq!(sm.router().route(b"z").unwrap(), 3);
        assert_eq!(sm.router().get_shard(2).unwrap().peers, vec![4, 5]);
    }

    // -----------------------------------------------------------------
    //  12. Phase 8.6：元数据 Raft 组集成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_meta_leader_election() {
        // 3 节点集群 → 元数据 Raft 组应选出 Leader
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 9100);

        cluster.run_for(1000);

        // 元数据 Leader 应存在
        assert!(cluster.meta_leader().is_some());
    }

    #[test]
    fn test_meta_propose_add_shard() {
        // propose AddShard 到元数据 Raft 组 → 所有节点的元数据状态机应更新
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![], 9200);

        cluster.run_for(1000);

        // propose 添加分片
        cluster
            .propose_metadata(MetadataOp::AddShard(Shard::new(
                10,
                KeyRange::unbounded(),
                vec![1, 2, 3],
            )))
            .unwrap();

        // 所有节点的元数据状态机应有分片 10
        for nid in [1, 2, 3] {
            let sm = cluster.meta_state_machine(nid).unwrap();
            assert_eq!(sm.shard_count(), 1);
            assert!(sm.router().get_shard(10).is_some());
        }
    }

    #[test]
    fn test_meta_split_recorded_in_log() {
        // split_shard 后，元数据操作应被记录到所有节点的元数据状态机
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 9300);

        cluster.run_for(1000);
        cluster.put(1, b"apple".to_vec(), b"red".to_vec()).unwrap();
        cluster.run_for(300);

        // 分裂
        let (id_left, id_right) = cluster.split_shard(1, b"m".to_vec(), 9400).unwrap();

        // 等待元数据 Raft 复制
        cluster.run_for(1000);

        // 所有节点的元数据状态机应反映分裂后的路由表
        for nid in [1, 2, 3] {
            let sm = cluster.meta_state_machine(nid).unwrap();
            // 旧分片 1 已移除，新分片 id_left 和 id_right 已添加
            assert!(
                sm.router().get_shard(1).is_none(),
                "node {} still has shard 1",
                nid
            );
            assert!(
                sm.router().get_shard(id_left).is_some(),
                "node {} missing shard {}",
                nid,
                id_left
            );
            assert!(
                sm.router().get_shard(id_right).is_some(),
                "node {} missing shard {}",
                nid,
                id_right
            );
            // 路由正确
            assert_eq!(sm.router().route(b"apple").unwrap(), id_left);
        }
    }

    #[test]
    fn test_meta_migrate_recorded_in_log() {
        // migrate_shard 后，元数据操作应被记录到所有节点的元数据状态机
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3, 4], vec![shard], 9500);

        cluster.run_for(1000);
        cluster.put(1, b"k1".to_vec(), b"v1".to_vec()).unwrap();
        cluster.run_for(300);

        // 迁移 [1,2,3] → [1,2,4]
        cluster.migrate_shard(1, vec![1, 2, 4], 9600).unwrap();

        // 等待元数据 Raft 复制
        cluster.run_for(1000);

        // 所有节点的元数据状态机应反映迁移后的 peers
        for nid in [1, 2, 3, 4] {
            let sm = cluster.meta_state_machine(nid).unwrap();
            let shard = sm.router().get_shard(1).unwrap();
            assert_eq!(shard.peers, vec![1, 2, 4], "node {} has wrong peers", nid);
        }
    }

    #[test]
    fn test_meta_recover_router_after_node_crash() {
        // Chaos：元数据节点崩溃 → 其他节点继续 propose → 崩溃节点恢复后从日志重建路由
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 9700);

        cluster.run_for(1000);

        // 节点 3 离线（模拟崩溃）
        cluster.set_offline(3);
        cluster.run_for(500);

        // 节点 3 离线期间，propose 添加新分片
        cluster
            .propose_metadata(MetadataOp::AddShard(Shard::new(
                99,
                KeyRange::new(b"z".to_vec(), b"~".to_vec()),
                vec![1, 2],
            )))
            .unwrap();

        // 节点 3 恢复
        cluster.set_online(3);
        cluster.run_for(2000);

        // 节点 3 的元数据状态机应从 Raft 日志追赶到最新状态
        let sm = cluster.meta_state_machine(3).unwrap();
        assert!(
            sm.router().get_shard(1).is_some(),
            "shard 1 should exist after recovery"
        );
        assert!(
            sm.router().get_shard(99).is_some(),
            "shard 99 should exist after recovery"
        );

        // recover_router 应返回与主路由器一致的路由表
        let recovered = cluster.recover_router(3).unwrap();
        assert!(recovered.get_shard(1).is_some());
        assert!(recovered.get_shard(99).is_some());
    }

    #[test]
    fn test_meta_recover_after_split_and_migrate() {
        // Chaos：先分裂再迁移 → 节点崩溃 → 恢复后路由信息完整
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3, 4], vec![shard], 9800);

        cluster.run_for(1000);

        // 写入数据并分裂
        for ch in b'a'..=b'z' {
            cluster.put(1, vec![ch], vec![ch]).unwrap();
        }
        cluster.run_for(500);
        let (id_left, id_right) = cluster.split_shard(1, b"m".to_vec(), 9900).unwrap();
        cluster.run_for(500);

        // 迁移右分片
        cluster
            .migrate_shard(id_right, vec![1, 2, 4], 9950)
            .unwrap();
        cluster.run_for(1000);

        // 节点 3 崩溃
        cluster.set_offline(3);
        cluster.run_for(500);

        // 节点 3 恢复
        cluster.set_online(3);
        cluster.run_for(2000);

        // 节点 3 的元数据状态机应反映所有变更
        let recovered = cluster.recover_router(3).unwrap();
        assert!(
            recovered.get_shard(1).is_none(),
            "old shard 1 should be gone"
        );
        assert!(
            recovered.get_shard(id_left).is_some(),
            "shard {} should exist",
            id_left
        );
        assert!(
            recovered.get_shard(id_right).is_some(),
            "shard {} should exist",
            id_right
        );

        // 验证路由正确
        assert_eq!(recovered.route(b"a").unwrap(), id_left);
        assert_eq!(recovered.route(b"z").unwrap(), id_right);

        // 验证迁移后的 peers
        let right_shard = recovered.get_shard(id_right).unwrap();
        assert_eq!(right_shard.peers, vec![1, 2, 4]);
    }

    #[test]
    fn test_meta_leader_failover() {
        // 元数据 Leader 崩溃 → 新 Leader 选举 → 继续 propose
        let shard = Shard::new(1, KeyRange::unbounded(), vec![1, 2, 3]);
        let mut cluster = ShardCluster::new(&[1, 2, 3], vec![shard], 10000);

        cluster.run_for(1000);
        let meta_leader = cluster.meta_leader().expect("meta leader should exist");

        // 元数据 Leader 崩溃
        cluster.set_offline(meta_leader);
        cluster.run_for(1000);

        // 新 Leader 应被选出
        let new_leader = cluster.meta_leader().expect("new meta leader should exist");
        assert_ne!(new_leader, meta_leader);

        // 新 Leader 可以继续 propose
        cluster
            .propose_metadata(MetadataOp::AddShard(Shard::new(
                50,
                KeyRange::new(b"z".to_vec(), b"~".to_vec()),
                vec![1, 2],
            )))
            .unwrap();

        // 恢复原 Leader
        cluster.set_online(meta_leader);
        cluster.run_for(2000);

        // 原 Leader 的元数据状态机应追赶到最新
        let sm = cluster.meta_state_machine(meta_leader).unwrap();
        assert!(sm.router().get_shard(50).is_some());
    }
}
