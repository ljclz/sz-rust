//! Phase 8.1 — Raft 共识算法（从零实现，不依赖 raft-rs/openraft）
//!
//! 参考 Ongaro & Ousterhout (2014) "In Search of an Understandable Consensus
//! Algorithm"（Raft 论文），从零自包含实现 Raft 共识算法。
//!
//! # 设计目标
//!
//! 1. **不依赖外部 Raft 库** — 避免 raft-rs 的 protobuf/gRPC 重依赖、openraft 的
//!    异步运行时耦合。本模块纯 Rust + std + serde，零网络依赖。
//! 2. **确定性** — 使用 LCG 伪随机数生成器（参考 `embedding.rs`），测试可复现。
//! 3. **逻辑时钟** — `tick(ms_elapsed)` 推进逻辑时钟，不依赖 tokio 实时定时器，
//!    便于确定性测试与高速模拟（100000 条日志复制可在秒级完成）。
//! 4. **论文忠实** — 严格遵循 Raft 论文 §5.2–§5.4 的选举、日志复制、安全性规则。
//!
//! # 架构
//!
//! - **RaftNode** — 单节点状态机，封装 Follower/Candidate/Leader 三态转换
//! - **RaftLog** — 日志存储（内存 Vec），支持 append/truncate/slice
//! - **RPC 消息** — RequestVote / AppendEntries 请求与响应（serde 可序列化）
//! - **RaftNetwork** — 网络抽象 trait，测试用 InMemoryNetwork 实现故障注入
//!
//! # 选举规则（§5.4.1）
//!
//! 1. RequestVote 的 term < current_term → 拒绝
//! 2. voted_for 为 None 或 candidate_id，且 candidate 日志至少和自己一样新 → 投票
//! 3. 日志新旧比较：(last_log_term, last_log_index) 字典序
//!
//! # 日志复制规则（§5.3 / §5.4.2）
//!
//! 1. AppendEntries 的 term < current_term → success=false
//! 2. prev_log_index > last_log_index → success=false（日志过短）
//! 3. log[prev_log_index].term != prev_log_term → success=false（任期不匹配）
//! 4. 冲突条目截断后追加新条目
//! 5. leader_commit > commit_index → commit_index = min(leader_commit, last_log_index)
//!
//! # Commit 安全性（§5.4.2）
//!
//! Leader 只 commit 当前 term 的日志条目，避免通过旧 term 日志 commit 导致
//! 已 commit 日志被覆盖的安全性违规。
//!
//! 对应 `SzRSQL实施进度.md` Phase 8.1。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{instrument, trace, warn};

// =====================================================================
//  类型别名
// =====================================================================

/// 节点 ID
pub type NodeId = u64;

/// 任期号
pub type Term = u64;

/// 日志索引（从 1 开始，0 表示空）
pub type Index = u64;

/// 日志任期
pub type LogTerm = u64;

// =====================================================================
//  常量
// =====================================================================

/// 默认选举超时下限（毫秒）
pub const DEFAULT_ELECTION_TIMEOUT_MIN_MS: u64 = 150;

/// 默认选举超时上限（毫秒）
pub const DEFAULT_ELECTION_TIMEOUT_MAX_MS: u64 = 300;

/// 默认心跳间隔（毫秒）
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 50;

/// 默认随机种子（确定性，可复现）
pub const DEFAULT_RAFT_SEED: u64 = 0x5EED_5EED_5EED_5EED;

// =====================================================================
//  Lcg — 确定性伪随机数生成器（参考 embedding.rs）
// =====================================================================

/// 线性同余生成器（PCG 风格，Numerical Recipes 常数）
///
/// 确定性、可复现，无需 `rand` crate 依赖。
/// 参考 `crates/szrsql-ai/src/embedding.rs` 中的实现。
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x6D2B_79F5),
        }
    }

    /// 返回 [0, 1) 区间均匀分布的 f64
    fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64) / (u32::MAX as f64 + 1.0)
    }

    /// 返回原始 u32 随机数，供 fuzz 测试选取节点编号等场景使用
    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let xorshifted = (((self.state >> 18) ^ self.state) >> 27) as u32;
        let rot = (self.state >> 59) as u32;
        (xorshifted >> rot) | (xorshifted << (rot.wrapping_neg() & 31))
    }
}

// =====================================================================
//  RaftError — 错误类型
// =====================================================================

/// Raft 共识算法错误
#[derive(Debug, Clone, Error)]
pub enum RaftError {
    /// 当前节点不是 Leader，无法处理 propose
    #[error("node {0} is not leader")]
    NotLeader(NodeId),

    /// 无效任期号
    #[error("invalid term: {0}")]
    InvalidTerm(Term),

    /// 日志条目未找到
    #[error("log entry not found at index {0}")]
    LogNotFound(Index),

    /// 配置错误
    #[error("config error: {0}")]
    ConfigError(String),

    /// 无效状态转换
    #[error("invalid state transition: {0}")]
    InvalidState(String),
}

// =====================================================================
//  LogEntry — 日志条目
// =====================================================================

/// Raft 日志条目
///
/// 每个条目包含产生它的任期号、在日志中的索引、命令载荷（字节序列）。
/// Phase 8.2 起新增 `config_change` 字段：当为 `Some` 时表示该条目为配置变更条目
/// （`Cold,new` 联合配置或 `Cnew` 新配置），需走联合共识两阶段提交流程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// 产生此条目的任期号
    pub term: Term,
    /// 日志索引（从 1 开始）
    pub index: Index,
    /// 命令载荷（通用字节序列；配置变更条目此处为空）
    pub command: Vec<u8>,
    /// 配置变更内容（`None`=普通数据条目，`Some`=配置变更条目）
    #[serde(default)]
    pub config_change: Option<ConfigChangeEntry>,
}

// =====================================================================
//  RaftLog — 日志存储
// =====================================================================

/// Raft 日志存储（内存实现）
///
/// 日志索引从 1 开始，`entries[0]` 对应 index=1，`entries[i]` 对应 index=i+1。
/// 索引 0 表示空（无日志）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RaftLog {
    /// 日志条目（按索引升序排列）
    entries: Vec<LogEntry>,
}

impl RaftLog {
    /// 创建空日志
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 返回最后一条日志的索引（空日志返回 0）
    pub fn last_log_index(&self) -> Index {
        self.entries.len() as u64
    }

    /// 返回最后一条日志的任期号（空日志返回 0）
    pub fn last_log_term(&self) -> LogTerm {
        self.entries.last().map(|e| e.term).unwrap_or(0)
    }

    /// 按索引获取日志条目（索引从 1 开始，0 返回 None）
    pub fn get(&self, index: Index) -> Option<&LogEntry> {
        if index == 0 {
            return None;
        }
        self.entries.get((index - 1) as usize)
    }

    /// 追加单条日志条目，返回新条目的索引
    pub fn append_entry(&mut self, term: Term, command: Vec<u8>) -> Index {
        self.append_entry_with_config(term, command, None)
    }

    /// 追加单条日志条目（含可选配置变更内容），返回新条目的索引
    ///
    /// `config_change` 为 `Some` 时表示该条目为配置变更条目（`Cold,new` 或 `Cnew`）。
    pub fn append_entry_with_config(
        &mut self,
        term: Term,
        command: Vec<u8>,
        config_change: Option<ConfigChangeEntry>,
    ) -> Index {
        let index = self.entries.len() as u64 + 1;
        self.entries.push(LogEntry {
            term,
            index,
            command,
            config_change,
        });
        index
    }

    /// 追加多条日志条目（条目索引必须连续递增）
    pub fn append_entries(&mut self, entries: Vec<LogEntry>) {
        for entry in entries {
            self.entries.push(entry);
        }
    }

    /// 截断指定索引之后的所有条目（保留 index 及之前的条目）
    ///
    /// `truncate_after(3)` 保留 index 1,2,3，删除 index 4+ 的条目。
    /// 若 index >= 当前长度，不做任何操作。
    pub fn truncate_after(&mut self, index: Index) {
        if index >= self.entries.len() as u64 {
            return;
        }
        self.entries.truncate(index as usize);
    }

    /// 返回 [from, to) 索引范围的日志条目切片（索引从 1 开始，to 为排他上界）
    ///
    /// `slice(1, 4)` 返回 index 1,2,3 的条目。`from=0` 等价于 `from=1`。
    pub fn slice(&self, from: Index, to: Index) -> &[LogEntry] {
        // from=0 等价于 from=1（index 0 不存在）
        let from = if from == 0 {
            1
        } else {
            from
        };
        // to=0 表示空范围
        if to == 0 || from >= to {
            return &[];
        }
        // log index i 对应 array[i-1]
        // [from, to) → array[from-1 .. to-1]
        let start = (from - 1) as usize;
        let end = ((to - 1) as usize).min(self.entries.len());
        if start >= end {
            return &[];
        }
        self.entries.get(start..end).unwrap_or(&[])
    }

    /// 返回所有日志条目
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// 返回日志条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 日志是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// =====================================================================
//  PersistentState — 持久化状态
// =====================================================================

/// 持久化状态（实际部署时需写入磁盘，测试中为内存）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentState {
    /// 当前任期号
    pub current_term: Term,
    /// 本任期投票对象（None 表示尚未投票）
    pub voted_for: Option<NodeId>,
    /// 日志
    pub log: RaftLog,
}

impl PersistentState {
    /// 创建初始持久化状态（term=0, voted_for=None, 空日志）
    pub fn new() -> Self {
        Self {
            current_term: 0,
            voted_for: None,
            log: RaftLog::new(),
        }
    }
}

impl Default for PersistentState {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  VolatileState — 易失状态
// =====================================================================

/// 易失状态（重启后丢失）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatileState {
    /// 已提交的最高日志索引
    pub commit_index: Index,
    /// 已应用到状态机的最高日志索引
    pub last_applied: Index,
}

impl VolatileState {
    /// 创建初始易失状态（commit_index=0, last_applied=0）
    pub fn new() -> Self {
        Self {
            commit_index: 0,
            last_applied: 0,
        }
    }
}

impl Default for VolatileState {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  LeaderState — Leader 专有状态
// =====================================================================

/// Leader 专有状态（仅 Leader 时存在）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LeaderState {
    /// 每个节点的下一条待发送日志索引
    pub next_index: HashMap<NodeId, Index>,
    /// 每个节点的已复制最高日志索引
    pub match_index: HashMap<NodeId, Index>,
}

// =====================================================================
//  Config — Raft 配置
// =====================================================================

/// Raft 配置
#[derive(Debug, Clone)]
pub struct Config {
    /// 集群中对端节点 ID 列表（不包含自身）
    pub peers: Vec<NodeId>,
    /// 选举超时下限（毫秒）
    pub election_timeout_min_ms: u64,
    /// 选举超时上限（毫秒）
    pub election_timeout_max_ms: u64,
    /// 心跳间隔（毫秒）
    pub heartbeat_interval_ms: u64,
    /// 随机种子（确定性，可复现）
    pub seed: u64,
}

impl Config {
    /// 创建配置
    pub fn new(peers: Vec<NodeId>) -> Self {
        Self {
            peers,
            election_timeout_min_ms: DEFAULT_ELECTION_TIMEOUT_MIN_MS,
            election_timeout_max_ms: DEFAULT_ELECTION_TIMEOUT_MAX_MS,
            heartbeat_interval_ms: DEFAULT_HEARTBEAT_INTERVAL_MS,
            seed: DEFAULT_RAFT_SEED,
        }
    }

    /// 创建单节点配置（无对端）
    pub fn single_node() -> Self {
        Self::new(Vec::new())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::single_node()
    }
}

// =====================================================================
//  MembershipChange — 成员变更（Phase 8.2 预留接口）
// =====================================================================

/// 集群成员变更类型（Raft 论文 §6 Joint Consensus）
///
/// Phase 8.1 仅提供接口与数据结构，实际两阶段提交流程在 Phase 8.2 实现。
/// 本枚举用于描述变更意图，便于上层调用方构造变更命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipChange {
    /// 添加单个节点
    AddNode(NodeId),
    /// 移除单个节点
    RemoveNode(NodeId),
    /// 联合共识（Joint Consensus）：旧配置 + 新配置同时生效
    ///
    /// Leader 先复制 `Cold,new` 联合配置，待多数派（新旧各占多数）提交后，
    /// 再复制 `Cnew` 新配置完成切换。本 Phase 8.1 stub 直接应用新配置。
    JointConsensus {
        /// 旧配置节点集合（含 Leader 自身）
        old_peers: Vec<NodeId>,
        /// 新配置节点集合（含 Leader 自身）
        new_peers: Vec<NodeId>,
    },
}

/// 成员变更阶段状态
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipChangeState {
    /// 稳定：无变更进行中
    #[default]
    Stable,
    /// 联合共识阶段：`Cold,new` 已写入但 `Cnew` 尚未提交
    JointConsensus,
    /// 变更已完成（`Cnew` 已提交）
    Completed,
}

// =====================================================================
//  Phase 8.2 — 联合共识（Joint Consensus）成员变更
// =====================================================================

/// 配置变更阶段（对应 Raft 论文 §6 两阶段提交的各阶段）
///
/// - `Joint`：`Cold,new` 联合配置条目（阶段 1）
/// - `New`：`Cnew` 新配置条目（阶段 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigStage {
    /// 联合配置 `Cold,new`（两阶段提交的第一阶段）
    Joint,
    /// 新配置 `Cnew`（两阶段提交的第二阶段）
    New,
}

/// 配置变更日志条目内容（嵌入 `LogEntry.config_change`）
///
/// 包含旧配置 `Cold`、新配置 `Cnew` 与变更阶段。
/// Leader 写入 `Cold,new` 联合配置后，待双多数派提交，再写入 `Cnew` 单独配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigChangeEntry {
    /// 旧配置节点集合 `Cold`（含 Leader 自身）
    pub old_peers: Vec<NodeId>,
    /// 新配置节点集合 `Cnew`（含 Leader 自身）
    pub new_peers: Vec<NodeId>,
    /// 变更阶段
    pub stage: ConfigStage,
}

impl ConfigChangeEntry {
    /// 创建 `Cold,new` 联合配置条目（阶段 1）
    pub fn joint(old_peers: Vec<NodeId>, new_peers: Vec<NodeId>) -> Self {
        Self {
            old_peers,
            new_peers,
            stage: ConfigStage::Joint,
        }
    }

    /// 创建 `Cnew` 新配置条目（阶段 2）
    pub fn new_config(new_peers: Vec<NodeId>, old_peers: Vec<NodeId>) -> Self {
        Self {
            old_peers,
            new_peers,
            stage: ConfigStage::New,
        }
    }
}

/// 联合共识进行中状态（Leader 与 Follower 内存中跟踪）
///
/// 当 `RaftNode.joint` 为 `Some` 时，集群处于联合共识阶段：
/// - `cnew_entry_index == 0`：`Cold,new` 已写入日志，待双多数派提交
/// - `cnew_entry_index > 0`：`Cnew` 已写入日志，待 Cnew 多数派提交
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JointConsensus {
    /// 旧配置 `Cold`（含 Leader 自身）
    pub cold: Vec<NodeId>,
    /// 新配置 `Cnew`（含 Leader 自身）
    pub cnew: Vec<NodeId>,
    /// `Cold,new` 联合配置条目的日志索引
    pub joint_entry_index: Index,
    /// `Cnew` 新配置条目的日志索引（0 表示尚未写入）
    pub cnew_entry_index: Index,
}

/// 计算给定节点集合是否构成多数派
///
/// `members` 中存在于 `replicated` 集合的节点数 > `members.len() / 2` 即为多数派。
/// 空配置永远返回 `false`（无法构成多数派）。
pub fn has_majority(replicated: &HashSet<NodeId>, members: &[NodeId]) -> bool {
    if members.is_empty() {
        return false;
    }
    let count = members.iter().filter(|m| replicated.contains(m)).count();
    count * 2 > members.len()
}

/// 计算联合共识双多数派（`Cold` 多数派 AND `Cnew` 多数派）
///
/// 联合共识阶段，任何提交都需要 `Cold` 与 `Cnew` 各自的多数派同时确认，
/// 防止旧 Leader 独自提交导致安全性违规。
pub fn has_joint_majority(replicated: &HashSet<NodeId>, cold: &[NodeId], cnew: &[NodeId]) -> bool {
    has_majority(replicated, cold) && has_majority(replicated, cnew)
}

// =====================================================================
//  RaftState — 节点角色
// =====================================================================

/// Raft 节点角色状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaftState {
    /// 跟随者
    Follower,
    /// 候选者
    Candidate,
    /// 领导者
    Leader,
}

impl std::fmt::Display for RaftState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RaftState::Follower => write!(f, "Follower"),
            RaftState::Candidate => write!(f, "Candidate"),
            RaftState::Leader => write!(f, "Leader"),
        }
    }
}

// =====================================================================
//  RPC 消息类型
// =====================================================================

/// RequestVote 请求（§5.4.1）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestVoteRequest {
    /// 候选者的任期号
    pub term: Term,
    /// 候选者 ID
    pub candidate_id: NodeId,
    /// 候选者最后一条日志索引
    pub last_log_index: Index,
    /// 候选者最后一条日志任期
    pub last_log_term: LogTerm,
}

/// RequestVote 响应
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestVoteResponse {
    /// 响应者的当前任期
    pub term: Term,
    /// 是否投票
    pub vote_granted: bool,
}

/// AppendEntries 请求（§5.3）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendEntriesRequest {
    /// 领导者任期
    pub term: Term,
    /// 领导者 ID
    pub leader_id: NodeId,
    /// 紧接新条目之前的日志索引
    pub prev_log_index: Index,
    /// prev_log_index 对应的任期
    pub prev_log_term: LogTerm,
    /// 待追加的日志条目（心跳时为空）
    pub entries: Vec<LogEntry>,
    /// 领导者的 commit_index
    pub leader_commit: Index,
}

/// AppendEntries 响应
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    /// 响应者的当前任期
    pub term: Term,
    /// 是否成功
    pub success: bool,
    /// 成功时为匹配到的最新索引；失败时为冲突提示索引（用于快速回退）
    pub match_index: Index,
}

/// RPC 消息类型枚举
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    /// RequestVote 请求
    RequestVoteRequest(RequestVoteRequest),
    /// RequestVote 响应
    RequestVoteResponse(RequestVoteResponse),
    /// AppendEntries 请求
    AppendEntriesRequest(AppendEntriesRequest),
    /// AppendEntries 响应
    AppendEntriesResponse(AppendEntriesResponse),
}

/// RPC 消息（含源/目的节点）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcMessage {
    /// 源节点
    pub from: NodeId,
    /// 目的节点
    pub to: NodeId,
    /// 消息内容
    pub message_type: MessageType,
}

impl RpcMessage {
    /// 创建 RPC 消息
    pub fn new(from: NodeId, to: NodeId, message_type: MessageType) -> Self {
        Self {
            from,
            to,
            message_type,
        }
    }
}

// =====================================================================
//  RaftNetwork — 网络抽象 trait
// =====================================================================

/// Raft 网络抽象
///
/// 生产环境可实现为真实网络层（TCP/gRPC）；测试环境使用 `InMemoryNetwork`。
pub trait RaftNetwork: Send + Sync {
    /// 发送消息（从 `from` 到 `to`）
    fn send(&self, from: NodeId, to: NodeId, msg: RpcMessage);
}

// =====================================================================
//  InMemoryNetwork — 内存网络（测试用，支持故障注入）
// =====================================================================

/// 内存网络（测试用）
///
/// 维护消息队列，支持节点离线、链路断开等故障注入。
/// 所有操作线程安全（`Mutex` 内部可变性）。
pub struct InMemoryNetwork {
    /// 待投递消息队列
    inbox: Mutex<Vec<RpcMessage>>,
    /// 离线节点集合（完全不可达）
    offline: Mutex<HashSet<NodeId>>,
    /// 断开链路集合（双向不可达）
    broken_links: Mutex<HashSet<(NodeId, NodeId)>>,
}

impl InMemoryNetwork {
    /// 创建空网络
    pub fn new() -> Self {
        Self {
            inbox: Mutex::new(Vec::new()),
            offline: Mutex::new(HashSet::new()),
            broken_links: Mutex::new(HashSet::new()),
        }
    }

    /// 设置节点离线
    pub fn set_offline(&self, node: NodeId) {
        if let Ok(mut offline) = self.offline.lock() {
            offline.insert(node);
        }
    }

    /// 设置节点上线
    pub fn set_online(&self, node: NodeId) {
        if let Ok(mut offline) = self.offline.lock() {
            offline.remove(&node);
        }
    }

    /// 节点是否离线
    pub fn is_offline(&self, node: NodeId) -> bool {
        self.offline
            .lock()
            .map(|offline| offline.contains(&node))
            .unwrap_or(false)
    }

    /// 两个节点之间是否被分区（双向不可达）
    pub fn is_partitioned(&self, a: NodeId, b: NodeId) -> bool {
        self.broken_links
            .lock()
            .map(|links| links.contains(&(a, b)) || links.contains(&(b, a)))
            .unwrap_or(false)
    }

    /// 断开两个节点之间的链路（双向）
    pub fn partition(&self, a: NodeId, b: NodeId) {
        if let Ok(mut links) = self.broken_links.lock() {
            links.insert((a, b));
            links.insert((b, a));
        }
    }

    /// 恢复两个节点之间的链路
    pub fn heal(&self, a: NodeId, b: NodeId) {
        if let Ok(mut links) = self.broken_links.lock() {
            links.remove(&(a, b));
            links.remove(&(b, a));
        }
    }

    /// 恢复所有链路和节点
    pub fn heal_all(&self) {
        if let Ok(mut offline) = self.offline.lock() {
            offline.clear();
        }
        if let Ok(mut links) = self.broken_links.lock() {
            links.clear();
        }
    }

    /// 检查链路是否可用
    fn link_available(&self, from: NodeId, to: NodeId) -> bool {
        if let Ok(links) = self.broken_links.lock() {
            !links.contains(&(from, to))
        } else {
            false
        }
    }

    /// 取出所有待投递消息（清空队列）
    pub fn drain(&self) -> Vec<RpcMessage> {
        if let Ok(mut inbox) = self.inbox.lock() {
            std::mem::take(&mut *inbox)
        } else {
            Vec::new()
        }
    }

    /// 待投递消息数
    pub fn pending_count(&self) -> usize {
        self.inbox.lock().map(|inbox| inbox.len()).unwrap_or(0)
    }
}

impl Default for InMemoryNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl RaftNetwork for InMemoryNetwork {
    fn send(&self, from: NodeId, to: NodeId, msg: RpcMessage) {
        // 离线节点不可收发
        if self.is_offline(from) || self.is_offline(to) {
            return;
        }
        // 断开链路丢弃消息
        if !self.link_available(from, to) {
            return;
        }
        if let Ok(mut inbox) = self.inbox.lock() {
            inbox.push(msg);
        }
    }
}

// =====================================================================
//  RaftNode — Raft 节点
// =====================================================================

/// Raft 节点状态机
///
/// 封装 Follower/Candidate/Leader 三态转换、选举、日志复制、提交推进。
/// 使用逻辑时钟（`tick(ms)`）推进时间，确定性 Lcg 随机化选举超时。
pub struct RaftNode {
    /// 节点 ID
    id: NodeId,
    /// Raft 配置
    config: Config,
    /// 当前角色
    state: RaftState,
    /// 持久化状态
    persistent: PersistentState,
    /// 易失状态
    volatile: VolatileState,
    /// Leader 专有状态（仅 Leader 时为 Some）
    leader_state: Option<LeaderState>,
    /// 选举超时截止时间（逻辑时间戳）
    election_deadline: u64,
    /// 上次心跳时间（逻辑时间戳）
    last_heartbeat: u64,
    /// 已收到投票的节点集合（仅 Candidate 时使用）
    votes_received: HashSet<NodeId>,
    /// 逻辑时钟当前时间
    current_time: u64,
    /// 确定性随机数生成器
    rng: Lcg,
    /// 成员变更阶段状态（Phase 8.2 预留）
    membership_state: MembershipChangeState,
    /// 联合共识进行中状态（Phase 8.2）
    ///
    /// - `None`：集群处于稳定配置，无变更进行中
    /// - `Some`：`Cold,new` 或 `Cnew` 配置条目已写入日志，正在等待多数派提交
    joint: Option<JointConsensus>,
}

impl RaftNode {
    /// 创建 Raft 节点
    ///
    /// 初始状态为 Follower，term=0，空日志。
    pub fn new(id: NodeId, config: Config) -> Self {
        let rng = Lcg::new(config.seed.wrapping_add(id));
        let mut node = Self {
            id,
            config,
            state: RaftState::Follower,
            persistent: PersistentState::new(),
            volatile: VolatileState::new(),
            leader_state: None,
            election_deadline: 0,
            last_heartbeat: 0,
            votes_received: HashSet::new(),
            current_time: 0,
            rng,
            membership_state: MembershipChangeState::Stable,
            joint: None,
        };
        node.reset_election_timer();
        node
    }

    // --- 查询方法 ---

    /// 返回节点 ID
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// 返回当前角色
    pub fn state(&self) -> RaftState {
        self.state
    }

    /// 返回当前任期
    pub fn current_term(&self) -> Term {
        self.persistent.current_term
    }

    /// 返回 commit_index
    pub fn commit_index(&self) -> Index {
        self.volatile.commit_index
    }

    /// 返回 last_applied
    pub fn last_applied(&self) -> Index {
        self.volatile.last_applied
    }

    /// 返回日志条目切片
    pub fn log_entries(&self) -> &[LogEntry] {
        self.persistent.log.entries()
    }

    /// 返回日志长度
    pub fn log_len(&self) -> usize {
        self.persistent.log.len()
    }

    /// 返回 last_log_index
    pub fn last_log_index(&self) -> Index {
        self.persistent.log.last_log_index()
    }

    /// 返回 last_log_term
    pub fn last_log_term(&self) -> LogTerm {
        self.persistent.log.last_log_term()
    }

    /// 返回 voted_for
    pub fn voted_for(&self) -> Option<NodeId> {
        self.persistent.voted_for
    }

    /// 返回当前逻辑时间
    pub fn current_time(&self) -> u64 {
        self.current_time
    }

    /// 返回 Leader 专有状态引用（仅 Leader 时为 Some）
    pub fn leader_state(&self) -> Option<&LeaderState> {
        self.leader_state.as_ref()
    }

    // --- 状态转换 ---

    /// 转为 Follower
    ///
    /// 若 `term` > current_term，更新 current_term 并清空 voted_for。
    /// 清理 votes_received 和 leader_state。
    #[instrument(skip(self), fields(node_id = self.id, term))]
    pub fn become_follower(&mut self, term: Term) {
        self.state = RaftState::Follower;
        if term > self.persistent.current_term {
            self.persistent.current_term = term;
            self.persistent.voted_for = None;
        }
        self.votes_received.clear();
        self.leader_state = None;
        self.reset_election_timer();
        tracing::Span::current().record("term", self.persistent.current_term);
        trace!(node_id = self.id, term = self.persistent.current_term, "became follower");
    }

    /// 转为 Candidate
    ///
    /// term+1，自投，清空 leader_state，重置选举定时器。
    #[instrument(skip(self), fields(node_id = self.id, term = self.persistent.current_term + 1))]
    pub fn become_candidate(&mut self) {
        self.state = RaftState::Candidate;
        self.persistent.current_term += 1;
        self.persistent.voted_for = Some(self.id);
        self.votes_received.clear();
        self.votes_received.insert(self.id);
        self.leader_state = None;
        self.reset_election_timer();
        trace!(node_id = self.id, term = self.persistent.current_term, "became candidate");
    }

    /// 转为 Leader
    ///
    /// 初始化 next_index/match_index，清空 votes_received。
    /// Phase 8.2：联合共识阶段下，需为 `Cold ∪ Cnew` 所有节点初始化复制状态。
    #[instrument(skip(self), fields(node_id = self.id, term = self.persistent.current_term))]
    pub fn become_leader(&mut self) {
        self.state = RaftState::Leader;
        self.votes_received.clear();
        let last_log_index = self.persistent.log.last_log_index();
        let mut next_index = HashMap::new();
        let mut match_index = HashMap::new();
        for peer in self.active_peers() {
            next_index.insert(peer, last_log_index + 1);
            match_index.insert(peer, 0);
        }
        self.leader_state = Some(LeaderState {
            next_index,
            match_index,
        });
        self.last_heartbeat = self.current_time;
    }

    // --- 选举定时器 ---

    /// 重置选举定时器（随机化超时，避免活锁）
    pub fn reset_election_timer(&mut self) {
        let range = self
            .config
            .election_timeout_max_ms
            .saturating_sub(self.config.election_timeout_min_ms);
        let rand_val = self.rng.next_f64();
        let timeout = self.config.election_timeout_min_ms + (rand_val * range as f64) as u64;
        self.election_deadline = self.current_time + timeout;
    }

    // --- 核心：tick ---

    /// 推进逻辑时钟，返回需要发送的 RPC 消息
    ///
    /// - Follower/Candidate：选举超时 → 成为 Candidate，发送 RequestVote
    /// - Leader：心跳间隔到期 → 发送 AppendEntries（含待复制日志）
    ///
    /// Phase 8.2：联合共识阶段下，Leader 心跳需发送到 `Cold ∪ Cnew` 所有节点。
    pub fn tick(&mut self, ms_elapsed: u64) -> Vec<RpcMessage> {
        self.current_time = self.current_time.saturating_add(ms_elapsed);
        let mut messages = Vec::new();

        match self.state {
            RaftState::Follower | RaftState::Candidate => {
                if self.current_time >= self.election_deadline {
                    self.become_candidate();
                    let req = RequestVoteRequest {
                        term: self.persistent.current_term,
                        candidate_id: self.id,
                        last_log_index: self.persistent.log.last_log_index(),
                        last_log_term: self.persistent.log.last_log_term(),
                    };
                    for peer in self.active_peers() {
                        messages.push(RpcMessage::new(
                            self.id,
                            peer,
                            MessageType::RequestVoteRequest(req.clone()),
                        ));
                    }
                }
            }
            RaftState::Leader => {
                if self.current_time >= self.last_heartbeat + self.config.heartbeat_interval_ms {
                    self.last_heartbeat = self.current_time;
                    for peer in self.active_peers() {
                        if let Some(msg) = self.build_append_entries(peer) {
                            messages.push(msg);
                        }
                    }
                }
            }
        }
        messages
    }

    // --- RPC 处理 ---

    /// 处理 RequestVote 请求（§5.4.1）
    pub fn handle_request_vote(&mut self, req: RequestVoteRequest) -> RequestVoteResponse {
        // 规则 1：term < current_term → 拒绝
        if req.term < self.persistent.current_term {
            return RequestVoteResponse {
                term: self.persistent.current_term,
                vote_granted: false,
            };
        }

        // term > current_term → 降级为 Follower
        if req.term > self.persistent.current_term {
            self.become_follower(req.term);
        }

        // 规则 2：检查是否可投票（voted_for 为 None 或 candidate_id）
        let can_vote = match self.persistent.voted_for {
            None => true,
            Some(voted) => voted == req.candidate_id,
        };

        if !can_vote {
            return RequestVoteResponse {
                term: self.persistent.current_term,
                vote_granted: false,
            };
        }

        // 规则 3：检查候选者日志是否至少和自己一样新
        let my_last_term = self.persistent.log.last_log_term();
        let my_last_index = self.persistent.log.last_log_index();
        let log_ok = req.last_log_term > my_last_term
            || (req.last_log_term == my_last_term && req.last_log_index >= my_last_index);

        if !log_ok {
            return RequestVoteResponse {
                term: self.persistent.current_term,
                vote_granted: false,
            };
        }

        // 投票
        self.persistent.voted_for = Some(req.candidate_id);
        self.reset_election_timer();

        RequestVoteResponse {
            term: self.persistent.current_term,
            vote_granted: true,
        }
    }

    /// 处理 AppendEntries 请求（§5.3 / §5.4.2）
    #[instrument(skip(self, req), fields(node_id = self.id, term = req.term, prev_log_index = req.prev_log_index, entries_count = req.entries.len(), success))]
    pub fn handle_append_entries(&mut self, req: AppendEntriesRequest) -> AppendEntriesResponse {
        // 规则 1：term < current_term → 拒绝
        if req.term < self.persistent.current_term {
            return AppendEntriesResponse {
                term: self.persistent.current_term,
                success: false,
                match_index: 0,
            };
        }

        // term > current_term 或当前非 Follower → 降级为 Follower
        // 两个分支动作相同（become_follower），合并为单一条件避免 if_same_then_else
        if req.term > self.persistent.current_term || self.state != RaftState::Follower {
            self.become_follower(req.term);
        }

        // 收到合法 Leader 心跳，重置选举定时器
        self.reset_election_timer();

        // 规则 2：prev_log_index > last_log_index → 日志过短
        let last_log_index = self.persistent.log.last_log_index();
        if req.prev_log_index > last_log_index {
            return AppendEntriesResponse {
                term: self.persistent.current_term,
                success: false,
                match_index: last_log_index,
            };
        }

        // 规则 3：prev_log_index > 0 时检查任期匹配
        if req.prev_log_index > 0 {
            if let Some(prev_entry) = self.persistent.log.get(req.prev_log_index) {
                if prev_entry.term != req.prev_log_term {
                    // 任期不匹配，返回冲突提示（prev_log_index - 1）
                    return AppendEntriesResponse {
                        term: self.persistent.current_term,
                        success: false,
                        match_index: req.prev_log_index.saturating_sub(1),
                    };
                }
            } else {
                // 不应发生（已检查 prev_log_index <= last_log_index）
                return AppendEntriesResponse {
                    term: self.persistent.current_term,
                    success: false,
                    match_index: last_log_index,
                };
            }
        }

        // 规则 4：追加条目，处理冲突
        let mut i = 0;
        while i < req.entries.len() {
            let entry = &req.entries[i];
            if entry.index <= self.persistent.log.last_log_index() {
                if let Some(existing) = self.persistent.log.get(entry.index) {
                    if existing.term != entry.term {
                        // 冲突：截断并追加此条及之后所有条目
                        self.persistent
                            .log
                            .truncate_after(entry.index.saturating_sub(1));
                        for entry in &req.entries[i..] {
                            self.persistent.log.append_entry_with_config(
                                entry.term,
                                entry.command.clone(),
                                entry.config_change.clone(),
                            );
                        }
                        break;
                    }
                    // 任期相同，已存在，继续检查下一条
                    i += 1;
                } else {
                    // 不应发生
                    i += 1;
                }
            } else {
                // 超出当前日志末尾，追加此条及之后所有条目
                for entry in &req.entries[i..] {
                    self.persistent.log.append_entry_with_config(
                        entry.term,
                        entry.command.clone(),
                        entry.config_change.clone(),
                    );
                }
                break;
            }
        }

        // Phase 8.2：处理配置变更条目，更新本地联合共识状态
        for entry in &req.entries {
            if let Some(config) = &entry.config_change {
                self.apply_config_change(config, entry.index);
            }
        }

        // 规则 5：更新 commit_index
        if req.leader_commit > self.volatile.commit_index {
            self.volatile.commit_index =
                req.leader_commit.min(self.persistent.log.last_log_index());
            // Phase 8.2：Cnew 提交后切换到新配置
            self.on_commit_advanced();
        }

        // 计算匹配索引
        let match_index = req
            .entries
            .last()
            .map(|e| e.index)
            .unwrap_or(req.prev_log_index);

        AppendEntriesResponse {
            term: self.persistent.current_term,
            success: true,
            match_index,
        }
    }

    /// 处理 RequestVote 响应（Candidate 状态）
    ///
    /// 获得多数票后成为 Leader，立即发送心跳。
    pub fn handle_request_vote_response(
        &mut self,
        from: NodeId,
        resp: RequestVoteResponse,
    ) -> Vec<RpcMessage> {
        let mut messages = Vec::new();

        // term > current_term → 降级
        if resp.term > self.persistent.current_term {
            self.become_follower(resp.term);
            return messages;
        }

        // 仅 Candidate 处理
        if self.state != RaftState::Candidate {
            return messages;
        }

        if resp.vote_granted {
            self.votes_received.insert(from);

            // Phase 8.2：联合共识阶段需 Cold 多数派 AND Cnew 多数派
            // 稳定阶段仅需单一多数派（向后兼容 Phase 8.1）
            let won = if let Some(joint) = &self.joint {
                has_joint_majority(&self.votes_received, &joint.cold, &joint.cnew)
            } else {
                let total_nodes = self.config.peers.len() + 1;
                let majority = total_nodes / 2 + 1;
                self.votes_received.len() >= majority
            };

            if won {
                self.become_leader();
                // 立即发送心跳
                for peer in self.active_peers() {
                    if let Some(msg) = self.build_append_entries(peer) {
                        messages.push(msg);
                    }
                }
            }
        }

        messages
    }

    /// 处理 AppendEntries 响应（Leader 状态）
    ///
    /// 成功：更新 match_index/next_index，推进 commit_index。
    /// 失败：回退 next_index，重发 AppendEntries。
    pub fn handle_append_entries_response(
        &mut self,
        from: NodeId,
        resp: AppendEntriesResponse,
    ) -> Vec<RpcMessage> {
        let mut messages = Vec::new();

        // term > current_term → 降级
        if resp.term > self.persistent.current_term {
            self.become_follower(resp.term);
            return messages;
        }

        // 仅 Leader 处理
        if self.state != RaftState::Leader {
            return messages;
        }

        if resp.success {
            if let Some(leader_state) = self.leader_state.as_mut() {
                leader_state.match_index.insert(from, resp.match_index);
                leader_state
                    .next_index
                    .insert(from, resp.match_index.saturating_add(1));
            }
            self.advance_commit();
        } else {
            // 快速回退：使用 match_index 提示
            if let Some(leader_state) = self.leader_state.as_mut() {
                let new_next = if resp.match_index > 0 {
                    resp.match_index + 1
                } else {
                    let current = leader_state.next_index.get(&from).copied().unwrap_or(1);
                    current.saturating_sub(1).max(1)
                };
                leader_state.next_index.insert(from, new_next);
            }
            // 立即重发
            if let Some(msg) = self.build_append_entries(from) {
                messages.push(msg);
            }
        }

        messages
    }

    // --- 客户端接口 ---

    /// Leader 接收客户端命令，追加到本地日志（尚未 commit）
    ///
    /// 返回新条目的索引。非 Leader 返回 `NotLeader` 错误。
    #[instrument(skip(self, command), fields(node_id = self.id, term = self.persistent.current_term, index))]
    pub fn propose(&mut self, command: Vec<u8>) -> Result<Index, RaftError> {
        if self.state != RaftState::Leader {
            warn!(node_id = self.id, "propose rejected: not leader");
            return Err(RaftError::NotLeader(self.id));
        }
        let index = self
            .persistent
            .log
            .append_entry(self.persistent.current_term, command);
        tracing::Span::current().record("index", index);
        trace!(node_id = self.id, index, "entry proposed");
        Ok(index)
    }

    // --- 提交推进 ---

    /// Leader 根据 match_index 推进 commit_index（§5.4.2 安全性）
    ///
    /// 找到最大的 N 满足：effective_config 多数派 match_index >= N 且
    /// log[N].term == current_term。
    ///
    /// Phase 8.2 扩展：联合共识阶段需同时满足 `Cold` 多数派 AND `Cnew` 多数派。
    /// 稳定配置阶段仅需单一多数派（向后兼容 Phase 8.1 行为）。
    /// 只 commit 当前 term 的日志条目（§5.4.2 安全性）。
    pub fn advance_commit(&mut self) {
        if self.state != RaftState::Leader {
            return;
        }
        let Some(leader_state) = self.leader_state.as_ref() else {
            return;
        };

        let self_last = self.persistent.log.last_log_index();
        if self_last == 0 {
            return;
        }

        // 计算当前生效配置（稳定时仅 Cold；联合共识时 Cold + Cnew）
        let (cold, cnew_opt) = self.effective_config();
        if cold.is_empty() {
            return;
        }

        // 从 commit_index+1 向后扫描，寻找满足多数派条件的最高 N
        let mut new_commit = self.volatile.commit_index;
        for n in (self.volatile.commit_index + 1)..=self_last {
            let entry = match self.persistent.log.get(n) {
                Some(e) => e,
                None => break,
            };
            // §5.4.2：只 commit 当前 term 的条目
            if entry.term != self.persistent.current_term {
                continue;
            }
            // 统计 Cold 多数派
            let cold_count = cold
                .iter()
                .filter(|&&p| {
                    if p == self.id {
                        self_last >= n
                    } else {
                        leader_state.match_index.get(&p).copied().unwrap_or(0) >= n
                    }
                })
                .count();
            let cold_ok = !cold.is_empty() && cold_count * 2 > cold.len();

            // 联合共识阶段还需 Cnew 多数派
            let cnew_ok = if let Some(cnew) = &cnew_opt {
                if cnew.is_empty() {
                    false
                } else {
                    let cnew_count = cnew
                        .iter()
                        .filter(|&&p| {
                            if p == self.id {
                                self_last >= n
                            } else {
                                leader_state.match_index.get(&p).copied().unwrap_or(0) >= n
                            }
                        })
                        .count();
                    cnew_count * 2 > cnew.len()
                }
            } else {
                true
            };

            if cold_ok && cnew_ok {
                new_commit = n;
            }
        }

        if new_commit > self.volatile.commit_index {
            self.volatile.commit_index = new_commit;
            self.on_leader_commit_advanced();
        }
    }

    // --- 应用日志 ---

    /// 返回新可应用的日志条目（commit_index > last_applied 的部分）
    ///
    /// 调用后 last_applied 更新为 commit_index。
    pub fn apply(&mut self) -> Vec<LogEntry> {
        let mut to_apply = Vec::new();
        while self.volatile.last_applied < self.volatile.commit_index {
            self.volatile.last_applied += 1;
            if let Some(entry) = self.persistent.log.get(self.volatile.last_applied) {
                to_apply.push(entry.clone());
            }
        }
        to_apply
    }

    // --- 内部辅助 ---

    /// 构建发往指定对端的 AppendEntries 消息
    fn build_append_entries(&self, peer: NodeId) -> Option<RpcMessage> {
        let leader_state = self.leader_state.as_ref()?;
        let next_index = leader_state.next_index.get(&peer).copied().unwrap_or(1);
        let last_log_index = self.persistent.log.last_log_index();

        let prev_log_index = next_index.saturating_sub(1);
        let prev_log_term = if prev_log_index == 0 {
            0
        } else {
            self.persistent
                .log
                .get(prev_log_index)
                .map(|e| e.term)
                .unwrap_or(0)
        };

        let entries: Vec<LogEntry> = if next_index <= last_log_index {
            self.persistent
                .log
                .slice(next_index, last_log_index + 1)
                .to_vec()
        } else {
            Vec::new()
        };

        Some(RpcMessage::new(
            self.id,
            peer,
            MessageType::AppendEntriesRequest(AppendEntriesRequest {
                term: self.persistent.current_term,
                leader_id: self.id,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit: self.volatile.commit_index,
            }),
        ))
    }

    // --- 成员变更（Phase 8.2 预留接口，本阶段为 stub） ---

    /// 添加节点到集群配置（§6 成员变更，Phase 8.2 stub）
    ///
    /// Phase 8.1 直接修改本地配置；Phase 8.2 应通过联合共识两阶段提交。
    /// 仅 Leader 可执行；重复添加或添加自身为无操作。
    pub fn add_node(&mut self, peer: NodeId) -> Result<(), RaftError> {
        if self.state != RaftState::Leader {
            return Err(RaftError::NotLeader(self.id));
        }
        if peer == self.id || self.config.peers.contains(&peer) {
            return Ok(());
        }
        self.config.peers.push(peer);
        let next_idx = self.last_log_index() + 1;
        if let Some(ls) = self.leader_state.as_mut() {
            ls.next_index.insert(peer, next_idx);
            ls.match_index.insert(peer, 0);
        }
        Ok(())
    }

    /// 从集群配置移除节点（§6 成员变更，Phase 8.2 stub）
    ///
    /// Phase 8.1 直接修改本地配置；Phase 8.2 应通过联合共识两阶段提交。
    /// 仅 Leader 可执行。
    pub fn remove_node(&mut self, peer: NodeId) -> Result<(), RaftError> {
        if self.state != RaftState::Leader {
            return Err(RaftError::NotLeader(self.id));
        }
        self.config.peers.retain(|&p| p != peer);
        if let Some(ls) = self.leader_state.as_mut() {
            ls.next_index.remove(&peer);
            ls.match_index.remove(&peer);
        }
        Ok(())
    }

    /// 发起联合共识成员变更（§6，Phase 8.2 stub）
    ///
    /// 完整实现应分两阶段：
    /// 1. Leader 写入 `Cold,new` 联合配置条目，待新旧配置各自多数派复制后提交；
    /// 2. Leader 写入 `Cnew` 新配置条目，待新配置多数派复制后提交，变更完成。
    ///
    /// Phase 8.1 stub 跳过两阶段流程，直接应用新配置（用于接口验证与单测）。
    pub fn propose_membership_change(&mut self, change: MembershipChange) -> Result<(), RaftError> {
        if self.state != RaftState::Leader {
            return Err(RaftError::NotLeader(self.id));
        }
        match change {
            MembershipChange::AddNode(peer) => {
                self.membership_state = MembershipChangeState::JointConsensus;
                self.add_node(peer)?;
                self.membership_state = MembershipChangeState::Completed;
            }
            MembershipChange::RemoveNode(peer) => {
                self.membership_state = MembershipChangeState::JointConsensus;
                self.remove_node(peer)?;
                self.membership_state = MembershipChangeState::Completed;
            }
            MembershipChange::JointConsensus { new_peers, .. } => {
                self.membership_state = MembershipChangeState::JointConsensus;
                // Stub：直接切换到新配置（去掉自身后作为 peers）
                self.config.peers = new_peers.into_iter().filter(|&p| p != self.id).collect();
                // 重建 Leader 复制状态
                let last_idx = self.last_log_index();
                if let Some(ls) = self.leader_state.as_mut() {
                    let mut new_next = HashMap::new();
                    let mut new_match = HashMap::new();
                    for &peer in &self.config.peers {
                        new_next.insert(peer, *ls.next_index.get(&peer).unwrap_or(&(last_idx + 1)));
                        new_match.insert(peer, *ls.match_index.get(&peer).unwrap_or(&0));
                    }
                    ls.next_index = new_next;
                    ls.match_index = new_match;
                }
                self.membership_state = MembershipChangeState::Completed;
            }
        }
        Ok(())
    }

    /// 返回当前集群成员列表（含自身，已排序）
    ///
    /// Phase 8.2：联合共识阶段下返回 `Cold ∪ Cnew`（去重后排序）。
    /// 稳定阶段返回 `self.id + config.peers`。
    pub fn cluster_members(&self) -> Vec<NodeId> {
        if let Some(joint) = &self.joint {
            let mut members = Vec::new();
            for &p in joint.cold.iter().chain(joint.cnew.iter()) {
                if !members.contains(&p) {
                    members.push(p);
                }
            }
            members.sort_unstable();
            members.dedup();
            members
        } else {
            let mut members = vec![self.id];
            members.extend(&self.config.peers);
            members.sort_unstable();
            members.dedup();
            members
        }
    }

    /// 返回当前成员变更阶段状态
    pub fn membership_state(&self) -> MembershipChangeState {
        self.membership_state
    }

    /// 返回对端列表（不含自身）
    pub fn peers(&self) -> &[NodeId] {
        &self.config.peers
    }

    /// 返回当前联合共识进行中状态（None=稳定）
    pub fn joint_consensus(&self) -> Option<&JointConsensus> {
        self.joint.as_ref()
    }

    // --- Phase 8.2 内部辅助方法 ---

    /// 返回当前需要复制的对端列表（不含自身）
    ///
    /// - 稳定阶段：`config.peers`
    /// - 联合共识阶段：`Cold ∪ Cnew` 去重后排除自身
    fn active_peers(&self) -> Vec<NodeId> {
        if let Some(joint) = &self.joint {
            let mut peers = Vec::new();
            for &p in joint.cold.iter().chain(joint.cnew.iter()) {
                if p != self.id && !peers.contains(&p) {
                    peers.push(p);
                }
            }
            peers
        } else {
            self.config.peers.clone()
        }
    }

    /// 返回当前生效配置（用于 commit 多数派计算）
    ///
    /// - 稳定阶段：`(cluster_members, None)` — 仅需单一多数派
    /// - 联合共识阶段：`(Cold, Some(Cnew))` — 需双多数派
    fn effective_config(&self) -> (Vec<NodeId>, Option<Vec<NodeId>>) {
        if let Some(joint) = &self.joint {
            (joint.cold.clone(), Some(joint.cnew.clone()))
        } else {
            (self.cluster_members(), None)
        }
    }

    /// 应用配置变更到本地联合共识状态
    ///
    /// 在 `handle_append_entries` 接收到配置变更条目后调用（Follower 端）。
    /// Leader 端在 `propose_membership_change_v2` 写入条目时直接设置 `self.joint`。
    fn apply_config_change(&mut self, config: &ConfigChangeEntry, index: Index) {
        match config.stage {
            ConfigStage::Joint => {
                // Cold,new 联合配置条目：进入联合共识阶段
                self.joint = Some(JointConsensus {
                    cold: config.old_peers.clone(),
                    cnew: config.new_peers.clone(),
                    joint_entry_index: index,
                    cnew_entry_index: 0,
                });
                self.membership_state = MembershipChangeState::JointConsensus;
            }
            ConfigStage::New => {
                // Cnew 新配置条目：更新 cnew_entry_index，等待提交
                if let Some(joint) = &mut self.joint {
                    joint.cnew_entry_index = index;
                }
            }
        }
    }

    /// Leader 提交推进后调用：检查是否需要写 Cnew 条目或切换到新配置
    fn on_leader_commit_advanced(&mut self) {
        // 如果 Cold,new 已提交且 Cnew 尚未写入，写入 Cnew 条目
        if let Some(joint) = self.joint.clone() {
            if joint.cnew_entry_index == 0 && self.volatile.commit_index >= joint.joint_entry_index
            {
                self.write_cnew_entry(joint);
            }
        }
        // 检查 Cnew 是否已提交
        self.on_commit_advanced();
    }

    /// 检查 Cnew 是否已提交，若是则切换到新配置
    ///
    /// Leader 和 Follower 都调用：Leader 在 `advance_commit` 后，
    /// Follower 在 `handle_append_entries` 收到 `leader_commit` 更新后。
    fn on_commit_advanced(&mut self) {
        if let Some(joint) = self.joint.clone() {
            if joint.cnew_entry_index > 0 && self.volatile.commit_index >= joint.cnew_entry_index {
                // Cnew 已提交：切换到新配置
                let cnew = joint.cnew.clone();
                let old_peers = joint.cold.clone();
                // 更新 config.peers 为 Cnew（排除自身）
                self.config.peers = cnew.iter().copied().filter(|&p| p != self.id).collect();
                // 清理 leader_state 中不在 Cnew 的对端
                if let Some(ls) = self.leader_state.as_mut() {
                    let to_remove: Vec<NodeId> = old_peers
                        .iter()
                        .filter(|p| !cnew.contains(p))
                        .copied()
                        .collect();
                    for p in to_remove {
                        ls.next_index.remove(&p);
                        ls.match_index.remove(&p);
                    }
                }
                self.joint = None;
                self.membership_state = MembershipChangeState::Stable;
            }
        }
    }

    /// 写入 Cnew 新配置条目（阶段 2）
    ///
    /// 在 `Cold,new` 提交后调用：构造 `Cnew` 配置条目追加到日志，
    /// 更新 `joint.cnew_entry_index`，等待 Cnew 多数派复制后提交。
    fn write_cnew_entry(&mut self, joint: JointConsensus) {
        let cnew_entry = ConfigChangeEntry::new_config(joint.cnew.clone(), joint.cold.clone());
        let cnew_index = self.persistent.log.append_entry_with_config(
            self.persistent.current_term,
            Vec::new(),
            Some(cnew_entry),
        );
        if let Some(j) = &mut self.joint {
            j.cnew_entry_index = cnew_index;
        }
    }

    // --- Phase 8.2 联合共识完整实现 ---

    /// 发起联合共识成员变更（Phase 8.2 完整实现）
    ///
    /// 实现 Raft 论文 §6 的两阶段提交：
    ///
    /// 1. 校验 Leader 身份与无进行中变更
    /// 2. 构造 `Cold,new` 联合配置条目，追加到日志
    /// 3. 复制到 `Cold ∪ Cnew` 所有节点，等待 Cold 多数派 + Cnew 多数派确认
    /// 4. `advance_commit` 提交 `Cold,new`，调用 `write_cnew_entry` 写入 `Cnew`
    /// 5. 复制 `Cnew` 到 Cnew 节点，等待 Cnew 多数派确认
    /// 6. `advance_commit` 提交 `Cnew`，调用 `on_commit_advanced` 切换配置
    /// 7. 不在 Cnew 中的节点被移除，变更完成
    ///
    /// 本方法仅完成步骤 1-2（写入 Cold,new）。步骤 3-7 通过 `tick`/`deliver_all`
    /// 循环自动推进，调用方需运行集群足够时间使变更完成。
    ///
    /// # 参数
    /// - `new_peers`：新配置节点列表（含自身；若不含自身会自动追加）
    ///
    /// # 错误
    /// - `NotLeader`：当前节点不是 Leader
    /// - `InvalidState`：已有成员变更进行中
    pub fn propose_membership_change_v2(
        &mut self,
        new_peers: Vec<NodeId>,
    ) -> Result<(), RaftError> {
        if self.state != RaftState::Leader {
            return Err(RaftError::NotLeader(self.id));
        }
        if self.joint.is_some() {
            return Err(RaftError::InvalidState(
                "membership change already in progress".to_string(),
            ));
        }

        // 构造 Cold（当前成员，含自身）
        let mut cold = self.cluster_members();
        cold.sort_unstable();
        cold.dedup();

        // 构造 Cnew（新成员，确保含自身）
        let mut cnew = new_peers;
        if !cnew.contains(&self.id) {
            cnew.push(self.id);
        }
        cnew.sort_unstable();
        cnew.dedup();

        // 配置未变化：无操作
        if cold == cnew {
            return Ok(());
        }

        // 构造 Cold,new 联合配置条目并追加到日志
        let entry = ConfigChangeEntry::joint(cold.clone(), cnew.clone());
        let joint_index = self.persistent.log.append_entry_with_config(
            self.persistent.current_term,
            Vec::new(),
            Some(entry),
        );

        // 设置联合共识状态
        self.joint = Some(JointConsensus {
            cold: cold.clone(),
            cnew: cnew.clone(),
            joint_entry_index: joint_index,
            cnew_entry_index: 0,
        });
        self.membership_state = MembershipChangeState::JointConsensus;

        // 为新加入节点（Cnew - Cold）初始化 Leader 复制状态
        // next_index = joint_index（让新节点从 Cold,new 条目开始复制）
        if let Some(ls) = self.leader_state.as_mut() {
            for &peer in &cnew {
                if !cold.contains(&peer) && peer != self.id {
                    ls.next_index.entry(peer).or_insert(joint_index);
                    ls.match_index.entry(peer).or_insert(0);
                }
            }
        }

        Ok(())
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    // -----------------------------------------------------------------
    //  测试辅助：Cluster — 多节点集群模拟器
    // -----------------------------------------------------------------

    /// 多节点集群模拟器（使用 InMemoryNetwork）
    struct Cluster {
        /// 节点集合
        nodes: HashMap<NodeId, RaftNode>,
        /// 内存网络
        network: InMemoryNetwork,
    }

    impl Cluster {
        /// 创建集群
        fn new(ids: &[NodeId], seed: u64) -> Self {
            let network = InMemoryNetwork::new();
            let mut nodes = HashMap::new();
            for &id in ids {
                let peers: Vec<NodeId> = ids.iter().copied().filter(|&p| p != id).collect();
                let config = Config {
                    peers,
                    election_timeout_min_ms: 150,
                    election_timeout_max_ms: 300,
                    heartbeat_interval_ms: 50,
                    seed,
                };
                nodes.insert(id, RaftNode::new(id, config));
            }
            Self { nodes, network }
        }

        /// 推进所有节点逻辑时钟，发送产生的消息
        ///
        /// 离线节点（崩溃）不处理 tick，模拟进程停止。
        fn tick(&mut self, ms: u64) {
            for (&id, node) in &mut self.nodes {
                if self.network.is_offline(id) {
                    continue;
                }
                let msgs = node.tick(ms);
                for msg in msgs {
                    self.network.send(msg.from, msg.to, msg);
                }
            }
        }

        /// 投递所有待处理消息（包括响应产生的消息），最多 200 轮
        fn deliver_all(&mut self) {
            for _ in 0..200 {
                let messages = self.network.drain();
                if messages.is_empty() {
                    break;
                }
                for msg in messages {
                    if let Some(target) = self.nodes.get_mut(&msg.to) {
                        let responses = match msg.message_type {
                            MessageType::RequestVoteRequest(req) => {
                                let resp = target.handle_request_vote(req);
                                vec![RpcMessage::new(
                                    msg.to,
                                    msg.from,
                                    MessageType::RequestVoteResponse(resp),
                                )]
                            }
                            MessageType::AppendEntriesRequest(req) => {
                                let resp = target.handle_append_entries(req);
                                vec![RpcMessage::new(
                                    msg.to,
                                    msg.from,
                                    MessageType::AppendEntriesResponse(resp),
                                )]
                            }
                            MessageType::RequestVoteResponse(resp) => {
                                target.handle_request_vote_response(msg.from, resp)
                            }
                            MessageType::AppendEntriesResponse(resp) => {
                                target.handle_append_entries_response(msg.from, resp)
                            }
                        };
                        for resp in responses {
                            self.network.send(resp.from, resp.to, resp);
                        }
                    }
                }
            }
        }

        /// 运行指定逻辑时间（步进 10ms），每步投递消息
        fn run_for(&mut self, total_ms: u64) {
            let step = 10u64;
            let mut elapsed = 0u64;
            while elapsed < total_ms {
                self.tick(step);
                self.deliver_all();
                elapsed += step;
            }
        }

        /// 返回当前在线 Leader（若有，排除离线节点）
        fn leader(&self) -> Option<NodeId> {
            self.nodes
                .iter()
                .filter(|(&id, _)| !self.network.is_offline(id))
                .find(|(_, n)| n.state() == RaftState::Leader)
                .map(|(&id, _)| id)
        }

        /// 设置节点离线
        fn set_offline(&self, node: NodeId) {
            self.network.set_offline(node);
        }

        /// 设置节点上线
        fn set_online(&self, node: NodeId) {
            self.network.set_online(node);
        }

        /// 分区两个节点
        fn partition(&self, a: NodeId, b: NodeId) {
            self.network.partition(a, b);
        }

        /// 恢复所有链路
        fn heal_all(&self) {
            self.network.heal_all();
        }

        /// 获取节点引用
        fn get(&self, id: NodeId) -> &RaftNode {
            self.nodes.get(&id).expect("node exists")
        }

        /// 获取节点可变引用
        fn get_mut(&mut self, id: NodeId) -> &mut RaftNode {
            self.nodes.get_mut(&id).expect("node exists")
        }
    }

    // -----------------------------------------------------------------
    //  1. 基础数据结构测试（8 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_raft_log_empty() {
        let log = RaftLog::new();
        assert_eq!(log.last_log_index(), 0);
        assert_eq!(log.last_log_term(), 0);
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.get(0).is_none());
        assert!(log.get(1).is_none());
    }

    #[test]
    fn test_raft_log_append() {
        let mut log = RaftLog::new();
        let idx1 = log.append_entry(1, vec![0x01]);
        let idx2 = log.append_entry(1, vec![0x02]);
        let idx3 = log.append_entry(2, vec![0x03]);
        assert_eq!(idx1, 1);
        assert_eq!(idx2, 2);
        assert_eq!(idx3, 3);
        assert_eq!(log.last_log_index(), 3);
        assert_eq!(log.len(), 3);
        assert!(!log.is_empty());
    }

    #[test]
    fn test_raft_log_get() {
        let mut log = RaftLog::new();
        log.append_entry(1, vec![0xAA]);
        log.append_entry(2, vec![0xBB]);
        log.append_entry(2, vec![0xCC]);

        assert!(log.get(0).is_none());
        let e1 = log.get(1).expect("entry 1");
        assert_eq!(e1.term, 1);
        assert_eq!(e1.index, 1);
        assert_eq!(e1.command, vec![0xAA]);

        let e3 = log.get(3).expect("entry 3");
        assert_eq!(e3.term, 2);
        assert_eq!(e3.index, 3);
        assert_eq!(e3.command, vec![0xCC]);

        assert!(log.get(4).is_none());
    }

    #[test]
    fn test_raft_log_truncate() {
        let mut log = RaftLog::new();
        log.append_entry(1, vec![0x01]);
        log.append_entry(1, vec![0x02]);
        log.append_entry(2, vec![0x03]);
        log.append_entry(2, vec![0x04]);

        // 截断 index 2 之后（保留 1,2）
        log.truncate_after(2);
        assert_eq!(log.last_log_index(), 2);
        assert!(log.get(3).is_none());
        assert_eq!(log.get(2).expect("entry 2").command, vec![0x02]);

        // 截断 index 0 之后（清空）
        log.truncate_after(0);
        assert_eq!(log.last_log_index(), 0);
        assert!(log.is_empty());

        // 截断超出范围不 panic
        log.truncate_after(100);
        assert_eq!(log.last_log_index(), 0);
    }

    #[test]
    fn test_raft_log_slice() {
        let mut log = RaftLog::new();
        log.append_entry(1, vec![0x01]);
        log.append_entry(1, vec![0x02]);
        log.append_entry(2, vec![0x03]);
        log.append_entry(2, vec![0x04]);
        log.append_entry(3, vec![0x05]);

        // slice(2, 5) → index 2,3,4
        let s = log.slice(2, 5);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].index, 2);
        assert_eq!(s[2].index, 4);

        // slice(1, 6) → 全部 5 条（to 被 clamp 到 5）
        let s = log.slice(1, 6);
        assert_eq!(s.len(), 5);

        // slice(3, 3) → 空
        let s = log.slice(3, 3);
        assert_eq!(s.len(), 0);

        // slice(0, 3) → index 1,2（from=0 → start=0）
        let s = log.slice(0, 3);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn test_raft_log_last_index_term() {
        let mut log = RaftLog::new();
        assert_eq!(log.last_log_index(), 0);
        assert_eq!(log.last_log_term(), 0);

        log.append_entry(1, vec![0x01]);
        assert_eq!(log.last_log_index(), 1);
        assert_eq!(log.last_log_term(), 1);

        log.append_entry(3, vec![0x02]);
        assert_eq!(log.last_log_index(), 2);
        assert_eq!(log.last_log_term(), 3);

        log.truncate_after(1);
        assert_eq!(log.last_log_index(), 1);
        assert_eq!(log.last_log_term(), 1);
    }

    #[test]
    fn test_log_entry_serde() {
        let entry = LogEntry {
            term: 42,
            index: 100,
            command: vec![0xDE, 0xAD, 0xBE, 0xEF],
            config_change: None,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let deserialized: LogEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.peers.is_empty());
        assert_eq!(
            config.election_timeout_min_ms,
            DEFAULT_ELECTION_TIMEOUT_MIN_MS
        );
        assert_eq!(
            config.election_timeout_max_ms,
            DEFAULT_ELECTION_TIMEOUT_MAX_MS
        );
        assert_eq!(config.heartbeat_interval_ms, DEFAULT_HEARTBEAT_INTERVAL_MS);
        assert_eq!(config.seed, DEFAULT_RAFT_SEED);

        let config2 = Config::new(vec![2, 3]);
        assert_eq!(config2.peers, vec![2, 3]);
        assert_eq!(config2.election_timeout_min_ms, 150);
        assert_eq!(config2.election_timeout_max_ms, 300);
    }

    // -----------------------------------------------------------------
    //  2. 选举测试（10 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_node_starts_as_follower() {
        let config = Config::single_node();
        let node = RaftNode::new(1, config);
        assert_eq!(node.state(), RaftState::Follower);
        assert_eq!(node.current_term(), 0);
        assert_eq!(node.commit_index(), 0);
        assert_eq!(node.last_applied(), 0);
        assert_eq!(node.last_log_index(), 0);
    }

    #[test]
    fn test_election_timeout_becomes_candidate() {
        let config = Config::new(vec![2, 3]);
        let mut node = RaftNode::new(1, config);
        assert_eq!(node.state(), RaftState::Follower);

        // 推进超过选举超时上限
        let msgs = node.tick(400);
        assert_eq!(node.state(), RaftState::Candidate);
        assert_eq!(node.current_term(), 1);
        // 应向 2 个对端发送 RequestVote
        assert_eq!(msgs.len(), 2);
        for msg in &msgs {
            assert_eq!(msg.from, 1);
            assert!(matches!(
                msg.message_type,
                MessageType::RequestVoteRequest(_)
            ));
        }
    }

    #[test]
    fn test_candidate_wins_election() {
        let config = Config::new(vec![2, 3]);
        let mut node = RaftNode::new(1, config);
        node.tick(400); // 成为 Candidate
        assert_eq!(node.state(), RaftState::Candidate);

        // 收到节点 2 和 3 的投票（2 票 + 自投 = 3，多数 = 2）
        let resp2 = RequestVoteResponse {
            term: 1,
            vote_granted: true,
        };
        let msgs = node.handle_request_vote_response(2, resp2);
        // 2 票（自投 + 节点2）≥ majority(2)，成为 Leader
        assert_eq!(node.state(), RaftState::Leader);
        // 成为 Leader 后应发送心跳
        assert_eq!(msgs.len(), 2); // 向 2 个对端发 AppendEntries
    }

    #[test]
    fn test_candidate_minority_votes() {
        // 5 节点集群，majority = 3
        let config = Config::new(vec![2, 3, 4, 5]);
        let mut node = RaftNode::new(1, config);
        node.tick(400); // 成为 Candidate
        assert_eq!(node.state(), RaftState::Candidate);

        // 只收到 1 票（自投 + 节点2 = 2 < 3）
        let resp = RequestVoteResponse {
            term: 1,
            vote_granted: true,
        };
        let _msgs = node.handle_request_vote_response(2, resp);
        assert_eq!(node.state(), RaftState::Candidate); // 仍为 Candidate
    }

    #[test]
    fn test_step_down_on_higher_term_request_vote() {
        let config = Config::new(vec![2]);
        let mut node = RaftNode::new(1, config);
        node.tick(400); // 成为 Candidate, term=1
        assert_eq!(node.state(), RaftState::Candidate);

        // 收到更高 term 的 RequestVote
        let req = RequestVoteRequest {
            term: 2,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        let resp = node.handle_request_vote(req);
        assert_eq!(node.state(), RaftState::Follower);
        assert_eq!(node.current_term(), 2);
        assert!(resp.vote_granted); // 日志一样新，投票
    }

    #[test]
    fn test_reject_old_term_request_vote() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        // 手动设置 term=5
        node.persistent.current_term = 5;

        let req = RequestVoteRequest {
            term: 3, // 旧 term
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        let resp = node.handle_request_vote(req);
        assert!(!resp.vote_granted);
        assert_eq!(resp.term, 5);
    }

    #[test]
    fn test_reject_stale_log_candidate() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.persistent.current_term = 1;
        // 节点 1 的日志：term=2, index=5
        node.persistent.log.append_entry(1, vec![0x01]);
        node.persistent.log.append_entry(2, vec![0x02]);
        node.persistent.log.append_entry(2, vec![0x03]);
        node.persistent.log.append_entry(2, vec![0x04]);
        node.persistent.log.append_entry(2, vec![0x05]);

        // 候选者日志更旧：term=1, index=3
        let req = RequestVoteRequest {
            term: 2,
            candidate_id: 2,
            last_log_index: 3,
            last_log_term: 1, // term < 2
        };
        let resp = node.handle_request_vote(req);
        assert!(!resp.vote_granted);
    }

    #[test]
    fn test_candidate_steps_down_on_higher_term_append() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.tick(400); // Candidate, term=1
        assert_eq!(node.state(), RaftState::Candidate);

        // 收到更高 term 的 AppendEntries
        let req = AppendEntriesRequest {
            term: 2,
            leader_id: 2,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        };
        let resp = node.handle_append_entries(req);
        assert_eq!(node.state(), RaftState::Follower);
        assert_eq!(node.current_term(), 2);
        assert!(resp.success);
    }

    #[test]
    fn test_reject_double_vote() {
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        node.persistent.current_term = 1;

        // 第一次投票给节点 2
        let req1 = RequestVoteRequest {
            term: 1,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        let resp1 = node.handle_request_vote(req1);
        assert!(resp1.vote_granted);

        // 同 term 第二次投票给节点 3 → 拒绝
        let req2 = RequestVoteRequest {
            term: 1,
            candidate_id: 3,
            last_log_index: 0,
            last_log_term: 0,
        };
        let resp2 = node.handle_request_vote(req2);
        assert!(!resp2.vote_granted);
    }

    #[test]
    fn test_election_timeout_randomized() {
        let config1 = Config::new(vec![2]);
        let config2 = Config::new(vec![2]);
        let mut node1 = RaftNode::new(1, config1);
        let node2 = RaftNode::new(2, config2);

        // 两个节点使用相同 seed 但不同 ID，选举超时应不同
        let deadline1 = node1.election_deadline;
        let deadline2 = node2.election_deadline;
        // 由于 Lcg seed = config.seed + id，不同 ID 产生不同超时
        assert_ne!(deadline1, deadline2);

        // 同一节点两次重置也应不同
        node1.reset_election_timer();
        let deadline1b = node1.election_deadline;
        assert_ne!(deadline1, deadline1b);
    }

    // -----------------------------------------------------------------
    //  3. 日志复制测试（12 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_leader_propose() {
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        node.become_candidate();
        node.become_leader();

        let idx1 = node.propose(vec![0x01]).expect("propose");
        let idx2 = node.propose(vec![0x02]).expect("propose");
        let idx3 = node.propose(vec![0x03]).expect("propose");

        assert_eq!(idx1, 1);
        assert_eq!(idx2, 2);
        assert_eq!(idx3, 3);
        assert_eq!(node.last_log_index(), 3);
        assert_eq!(node.log_entries().len(), 3);
    }

    #[test]
    fn test_append_entries_success() {
        let mut follower = RaftNode::new(2, Config::new(vec![1]));

        let entries = vec![
            LogEntry {
                term: 1,
                index: 1,
                command: vec![0x01],
                config_change: None,
            },
            LogEntry {
                term: 1,
                index: 2,
                command: vec![0x02],
                config_change: None,
            },
        ];
        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries,
            leader_commit: 0,
        };
        let resp = follower.handle_append_entries(req);
        assert!(resp.success);
        assert_eq!(resp.match_index, 2);
        assert_eq!(follower.last_log_index(), 2);
    }

    #[test]
    fn test_append_entries_log_too_short() {
        let mut follower = RaftNode::new(2, Config::new(vec![1]));
        // Follower 日志为空
        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 5, // 超出空日志
            prev_log_term: 1,
            entries: vec![],
            leader_commit: 0,
        };
        let resp = follower.handle_append_entries(req);
        assert!(!resp.success);
        assert_eq!(resp.match_index, 0); // 提示 last_log_index=0
    }

    #[test]
    fn test_append_entries_term_mismatch_truncate() {
        let mut follower = RaftNode::new(2, Config::new(vec![1]));
        // Follower 已有 term=2 的 entry at index 1
        follower.persistent.log.append_entry(2, vec![0xFF]);

        // Leader 发送 term=1 的 entry at index 1（冲突）
        let entries = vec![LogEntry {
            term: 1,
            index: 1,
            command: vec![0x01],
            config_change: None,
        }];
        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries,
            leader_commit: 0,
        };
        let resp = follower.handle_append_entries(req);
        assert!(resp.success);
        // 冲突条目应被截断并替换
        assert_eq!(follower.log_entries().len(), 1);
        assert_eq!(follower.log_entries()[0].term, 1);
        assert_eq!(follower.log_entries()[0].command, vec![0x01]);
    }

    #[test]
    fn test_leader_updates_match_index_on_success() {
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        leader.propose(vec![0x01]).expect("propose");
        leader.propose(vec![0x02]).expect("propose");

        // 收到 Follower 2 的成功响应
        let resp = AppendEntriesResponse {
            term: 1,
            success: true,
            match_index: 2,
        };
        let _msgs = leader.handle_append_entries_response(2, resp);

        let ls = leader.leader_state().expect("leader state");
        assert_eq!(ls.match_index.get(&2), Some(&2));
        assert_eq!(ls.next_index.get(&2), Some(&3));
    }

    #[test]
    fn test_leader_decrements_next_index_on_fail() {
        let mut leader = RaftNode::new(1, Config::new(vec![2]));
        leader.become_candidate();
        leader.become_leader();
        leader.propose(vec![0x01]).expect("propose");
        leader.propose(vec![0x02]).expect("propose");
        leader.propose(vec![0x03]).expect("propose");

        // next_index 初始为 1（last_log_index+1 = 4 → 不对，初始为 last_log_index+1=4?）
        // 不，become_leader 时 next_index = last_log_index+1。此时 log 为空 → next_index=1
        // propose 后 log 有 3 条，但 next_index 仍为 1（没更新）
        // 初始 next_index = 0 + 1 = 1
        let ls = leader.leader_state().expect("leader state");
        assert_eq!(ls.next_index.get(&2), Some(&1));

        // 模拟 Follower 返回 fail（日志过短），match_index 提示 = 0
        let resp = AppendEntriesResponse {
            term: 1,
            success: false,
            match_index: 0,
        };
        let msgs = leader.handle_append_entries_response(2, resp);

        // next_index 应回退到 1（已经是 1，不变），并重发
        let ls = leader.leader_state().expect("leader state");
        assert_eq!(ls.next_index.get(&2), Some(&1));
        // 应产生重发消息
        assert!(!msgs.is_empty());
    }

    #[test]
    fn test_leader_advances_commit() {
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        leader.propose(vec![0x01]).expect("propose");

        // 两个 Follower 都成功复制 index=1
        let resp2 = AppendEntriesResponse {
            term: 1,
            success: true,
            match_index: 1,
        };
        let resp3 = AppendEntriesResponse {
            term: 1,
            success: true,
            match_index: 1,
        };
        let _m2 = leader.handle_append_entries_response(2, resp2);
        let _m3 = leader.handle_append_entries_response(3, resp3);

        // majority(2) match_index >= 1, log[1].term == 1 == current_term → commit
        assert_eq!(leader.commit_index(), 1);
    }

    #[test]
    fn test_leader_only_commits_current_term() {
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        // 模拟旧 term 日志：term=1 的 2 条
        leader.persistent.log.append_entry(1, vec![0x01]);
        leader.persistent.log.append_entry(1, vec![0x02]);
        // 当前 term=2
        leader.persistent.current_term = 2;
        leader.state = RaftState::Leader;
        leader.leader_state = Some(LeaderState {
            next_index: HashMap::from([(2, 1), (3, 1)]),
            match_index: HashMap::from([(2, 0), (3, 0)]),
        });

        // 两个 Follower 都复制到 index=2（旧 term 日志）
        let resp2 = AppendEntriesResponse {
            term: 2,
            success: true,
            match_index: 2,
        };
        let resp3 = AppendEntriesResponse {
            term: 2,
            success: true,
            match_index: 2,
        };
        let _m2 = leader.handle_append_entries_response(2, resp2);
        let _m3 = leader.handle_append_entries_response(3, resp3);

        // majority match_index >= 2，但 log[2].term=1 != current_term=2
        // 不应 commit index=2
        assert_eq!(leader.commit_index(), 0);

        // 现在 propose 一条 term=2 的日志
        leader.propose(vec![0x03]).expect("propose"); // index=3, term=2
                                                      // Follower 复制到 index=3
        let resp2b = AppendEntriesResponse {
            term: 2,
            success: true,
            match_index: 3,
        };
        let resp3b = AppendEntriesResponse {
            term: 2,
            success: true,
            match_index: 3,
        };
        let _m2b = leader.handle_append_entries_response(2, resp2b);
        let _m3b = leader.handle_append_entries_response(3, resp3b);

        // log[3].term=2 == current_term=2 → commit index=3
        // （此时 index 1,2 也一并被 commit，因为 commit_index 单调递增）
        assert_eq!(leader.commit_index(), 3);
    }

    #[test]
    fn test_follower_updates_commit() {
        let mut follower = RaftNode::new(2, Config::new(vec![1]));
        // Follower 有 3 条日志
        follower.persistent.log.append_entry(1, vec![0x01]);
        follower.persistent.log.append_entry(1, vec![0x02]);
        follower.persistent.log.append_entry(1, vec![0x03]);

        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 3,
            prev_log_term: 1,
            entries: vec![],
            leader_commit: 2, // Leader 已 commit 到 2
        };
        let resp = follower.handle_append_entries(req);
        assert!(resp.success);
        assert_eq!(follower.commit_index(), 2); // min(2, 3) = 2
    }

    #[test]
    fn test_heartbeat_maintains_leadership() {
        let mut follower = RaftNode::new(2, Config::new(vec![1]));
        // 空 AppendEntries（心跳）
        let req = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        };
        let resp = follower.handle_append_entries(req);
        assert!(resp.success);
        assert_eq!(resp.match_index, 0);
    }

    #[test]
    fn test_old_leader_heartbeat_rejected() {
        let mut node = RaftNode::new(2, Config::new(vec![1]));
        node.persistent.current_term = 5; // 当前 term 更高

        // 旧 Leader（term=3）的心跳
        let req = AppendEntriesRequest {
            term: 3,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        };
        let resp = node.handle_append_entries(req);
        assert!(!resp.success);
        assert_eq!(resp.term, 5);
    }

    #[test]
    fn test_log_consistency_repair() {
        let mut follower = RaftNode::new(2, Config::new(vec![1]));
        // Follower 日志：[1(t1), 2(t1), 3(t2)]（index 3 的 term 不匹配 Leader）
        follower.persistent.log.append_entry(1, vec![0x01]);
        follower.persistent.log.append_entry(1, vec![0x02]);
        follower.persistent.log.append_entry(2, vec![0xFF]); // 冲突

        // Leader 日志：[1(t1), 2(t1), 3(t1)]
        // 第一次尝试：prev_log_index=3, prev_log_term=1 → Follower log[3].term=2 不匹配
        let req1 = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 3,
            prev_log_term: 1,
            entries: vec![LogEntry {
                term: 1,
                index: 3,
                command: vec![0x03],
                config_change: None,
            }],
            leader_commit: 0,
        };
        let resp1 = follower.handle_append_entries(req1);
        assert!(!resp1.success);
        assert_eq!(resp1.match_index, 2); // 提示回退到 index 2

        // 第二次：prev_log_index=2, prev_log_term=1 → 匹配
        let req2 = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 2,
            prev_log_term: 1,
            entries: vec![LogEntry {
                term: 1,
                index: 3,
                command: vec![0x03],
                config_change: None,
            }],
            leader_commit: 0,
        };
        let resp2 = follower.handle_append_entries(req2);
        assert!(resp2.success);
        assert_eq!(resp2.match_index, 3);

        // 验证冲突条目被替换
        assert_eq!(follower.log_entries().len(), 3);
        assert_eq!(follower.log_entries()[2].term, 1);
        assert_eq!(follower.log_entries()[2].command, vec![0x03]);
    }

    // -----------------------------------------------------------------
    //  4. 状态转换测试（5 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_follower_to_candidate() {
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        assert_eq!(node.state(), RaftState::Follower);
        node.tick(400); // 选举超时
        assert_eq!(node.state(), RaftState::Candidate);
        assert_eq!(node.current_term(), 1);
    }

    #[test]
    fn test_candidate_to_leader() {
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        node.become_candidate();
        assert_eq!(node.state(), RaftState::Candidate);

        let resp = RequestVoteResponse {
            term: 1,
            vote_granted: true,
        };
        let _msgs = node.handle_request_vote_response(2, resp);
        assert_eq!(node.state(), RaftState::Leader);
        assert!(node.leader_state().is_some());
    }

    #[test]
    fn test_candidate_to_follower_higher_term() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.become_candidate();
        assert_eq!(node.state(), RaftState::Candidate);

        // 收到更高 term 的 RequestVote
        let req = RequestVoteRequest {
            term: 10,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        let _resp = node.handle_request_vote(req);
        assert_eq!(node.state(), RaftState::Follower);
        assert_eq!(node.current_term(), 10);
    }

    #[test]
    fn test_leader_to_follower_higher_term() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.become_candidate();
        node.become_leader();
        assert_eq!(node.state(), RaftState::Leader);

        // 收到更高 term 的 AppendEntries
        let req = AppendEntriesRequest {
            term: 10,
            leader_id: 2,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        };
        let _resp = node.handle_append_entries(req);
        assert_eq!(node.state(), RaftState::Follower);
        assert_eq!(node.current_term(), 10);
        assert!(node.leader_state().is_none());
    }

    #[test]
    fn test_state_transition_clears_temp() {
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        node.become_candidate();
        // votes_received 应含自投
        assert_eq!(node.votes_received.len(), 1);

        // 成为 Leader → 清空 votes_received
        node.become_leader();
        assert!(node.votes_received.is_empty());
        assert!(node.leader_state.is_some());

        // 降级为 Follower → 清空 leader_state
        node.become_follower(node.current_term());
        assert!(node.leader_state.is_none());
        assert!(node.votes_received.is_empty());
    }

    // -----------------------------------------------------------------
    //  5. 应用日志测试（3 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_apply_returns_committed() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.persistent.log.append_entry(1, vec![0x01]);
        node.persistent.log.append_entry(1, vec![0x02]);
        node.persistent.log.append_entry(1, vec![0x03]);
        node.volatile.commit_index = 2;

        let applied = node.apply();
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0].index, 1);
        assert_eq!(applied[1].index, 2);
        assert_eq!(applied[0].command, vec![0x01]);
        assert_eq!(applied[1].command, vec![0x02]);
    }

    #[test]
    fn test_apply_updates_last_applied() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.persistent.log.append_entry(1, vec![0x01]);
        node.persistent.log.append_entry(1, vec![0x02]);
        node.volatile.commit_index = 2;

        assert_eq!(node.last_applied(), 0);
        let _applied = node.apply();
        assert_eq!(node.last_applied(), 2);

        // 再次 apply 应返回空
        let applied2 = node.apply();
        assert!(applied2.is_empty());
    }

    #[test]
    fn test_apply_empty_when_no_commit() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        node.persistent.log.append_entry(1, vec![0x01]);
        node.volatile.commit_index = 0; // 未 commit

        let applied = node.apply();
        assert!(applied.is_empty());
        assert_eq!(node.last_applied(), 0);
    }

    // -----------------------------------------------------------------
    //  6. 端到端集成测试（5 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_three_node_election() {
        let mut cluster = Cluster::new(&[1, 2, 3], 42);

        // 运行足够时间让选举完成（400ms > 选举超时上限 300ms）
        cluster.run_for(500);

        // 应选出恰好 1 个 Leader
        let leaders: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(_, n)| n.state() == RaftState::Leader)
            .map(|(&id, _)| id)
            .collect();
        assert_eq!(leaders.len(), 1, "应有且仅有 1 个 Leader");

        let leader = leaders[0];
        let followers: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(_, n)| n.state() == RaftState::Follower)
            .map(|(&id, _)| id)
            .collect();
        assert_eq!(followers.len(), 2, "应有 2 个 Follower");

        // 所有节点 term 应一致
        let leader_term = cluster.get(leader).current_term();
        for node in cluster.nodes.values() {
            assert_eq!(node.current_term(), leader_term);
        }
    }

    #[test]
    fn test_100_entries_replicated() {
        let mut cluster = Cluster::new(&[1, 2, 3], 100);
        cluster.run_for(500); // 选举

        let leader = cluster.leader().expect("leader elected");
        let term = cluster.get(leader).current_term();

        // Leader 写入 100 条日志
        for i in 0..100u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }

        // 运行足够时间让复制完成
        cluster.run_for(300);

        // 所有节点日志应一致（100 条）
        for (&id, node) in &cluster.nodes {
            assert_eq!(
                node.log_len(),
                100,
                "node {} should have 100 entries, got {}",
                id,
                node.log_len()
            );
            assert_eq!(node.current_term(), term);
        }

        // Leader commit_index 应为 100
        assert_eq!(cluster.get(leader).commit_index(), 100);

        // 所有节点 commit_index 应为 100（通过心跳传播）
        for node in cluster.nodes.values() {
            assert_eq!(node.commit_index(), 100);
        }
    }

    #[test]
    fn test_100000_entries_replicated() {
        let start = Instant::now();

        let mut cluster = Cluster::new(&[1, 2, 3], 999);
        cluster.run_for(500); // 选举

        let leader = cluster.leader().expect("leader elected");

        // Leader 写入 100000 条日志
        for i in 0..100000u32 {
            cluster
                .get_mut(leader)
                .propose(i.to_le_bytes().to_vec())
                .expect("propose");
        }

        // 运行足够时间让复制完成（增加轮次确保完成）
        cluster.run_for(1000);

        let elapsed = start.elapsed();

        // 所有节点日志应一致（100000 条）
        for (&id, node) in &cluster.nodes {
            assert_eq!(
                node.log_len(),
                100000,
                "node {} should have 100000 entries, got {}",
                id,
                node.log_len()
            );
        }

        // Leader commit_index 应为 100000
        assert_eq!(cluster.get(leader).commit_index(), 100000);

        // 验证数据完整性：检查最后一条
        let last_entry = cluster
            .get(leader)
            .log_entries()
            .last()
            .expect("last entry");
        assert_eq!(last_entry.command, 99999u32.to_le_bytes().to_vec());

        eprintln!("100000 条日志复制完成，耗时: {:.2}s", elapsed.as_secs_f64());

        // 应在 120s 内完成
        assert!(
            elapsed.as_secs() < 120,
            "100000 entries replication took too long: {:.2}s",
            elapsed.as_secs_f64()
        );
    }

    #[test]
    fn test_leader_crash_reelection() {
        let mut cluster = Cluster::new(&[1, 2, 3], 200);
        cluster.run_for(500); // 选举

        let old_leader = cluster.leader().expect("leader elected");

        // Leader 写入一些日志
        for i in 0..10u8 {
            cluster
                .get_mut(old_leader)
                .propose(vec![i])
                .expect("propose");
        }
        cluster.run_for(300);

        // 验证日志已复制
        for node in cluster.nodes.values() {
            assert_eq!(node.log_len(), 10);
        }

        // Leader 崩溃（从网络移除）
        cluster.set_offline(old_leader);

        // 运行足够时间让新选举发生
        cluster.run_for(1000);

        // 应选出新 Leader（不是 old_leader）
        let new_leader = cluster.leader();
        assert!(new_leader.is_some(), "应选出新 Leader");
        assert_ne!(
            new_leader,
            Some(old_leader),
            "新 Leader 不应是崩溃的旧 Leader"
        );

        // 日志不丢失（剩余 2 个节点仍有 10 条日志）
        let new_leader_id = new_leader.unwrap();
        assert_eq!(cluster.get(new_leader_id).log_len(), 10);

        // 另一个存活的 Follower 也有 10 条
        for (&id, node) in &cluster.nodes {
            if id != old_leader {
                assert_eq!(node.log_len(), 10, "node {} log not lost", id);
            }
        }
    }

    #[test]
    fn test_network_partition_and_heal() {
        let mut cluster = Cluster::new(&[1, 2], 300);
        // 初始不运行选举，直接分区

        // 分区两个节点
        cluster.partition(1, 2);

        // 运行一段时间，由于分区，两个节点都无法收到对方的投票
        cluster.run_for(1000);

        // 无 Leader（2 节点需要 2 票 = majority，分区下无法获得）
        assert!(
            cluster.leader().is_none(),
            "分区期间不应有 Leader（无多数）"
        );

        // 恢复分区
        cluster.heal_all();

        // 运行足够时间让选举发生
        cluster.run_for(1000);

        // 应选出 Leader
        assert!(cluster.leader().is_some(), "分区恢复后应选出 Leader");
    }

    // -----------------------------------------------------------------
    //  补充测试：RaftError 变体
    // -----------------------------------------------------------------

    #[test]
    fn test_propose_on_non_leader_returns_error() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        // Follower 状态
        let result = node.propose(vec![0x01]);
        assert!(matches!(result, Err(RaftError::NotLeader(1))));
    }

    #[test]
    fn test_persistent_state_default() {
        let ps = PersistentState::default();
        assert_eq!(ps.current_term, 0);
        assert!(ps.voted_for.is_none());
        assert!(ps.log.is_empty());
    }

    #[test]
    fn test_volatile_state_default() {
        let vs = VolatileState::default();
        assert_eq!(vs.commit_index, 0);
        assert_eq!(vs.last_applied, 0);
    }

    #[test]
    fn test_leader_state_default() {
        let ls = LeaderState::default();
        assert!(ls.next_index.is_empty());
        assert!(ls.match_index.is_empty());
    }

    #[test]
    fn test_inmemory_network_offline() {
        let net = InMemoryNetwork::new();
        assert!(!net.is_offline(1));

        net.set_offline(1);
        assert!(net.is_offline(1));

        // 离线节点发送的消息被丢弃
        net.send(
            1,
            2,
            RpcMessage::new(
                1,
                2,
                MessageType::RequestVoteResponse(RequestVoteResponse {
                    term: 1,
                    vote_granted: true,
                }),
            ),
        );
        assert_eq!(net.pending_count(), 0);

        net.set_online(1);
        assert!(!net.is_offline(1));

        // 上线后消息正常入队
        net.send(
            1,
            2,
            RpcMessage::new(
                1,
                2,
                MessageType::RequestVoteResponse(RequestVoteResponse {
                    term: 1,
                    vote_granted: true,
                }),
            ),
        );
        assert_eq!(net.pending_count(), 1);
    }

    #[test]
    fn test_inmemory_network_partition() {
        let net = InMemoryNetwork::new();
        net.partition(1, 2);

        // 分区后消息被丢弃
        net.send(
            1,
            2,
            RpcMessage::new(
                1,
                2,
                MessageType::RequestVoteResponse(RequestVoteResponse {
                    term: 1,
                    vote_granted: true,
                }),
            ),
        );
        assert_eq!(net.pending_count(), 0);

        // 恢复后消息正常
        net.heal(1, 2);
        net.send(
            1,
            2,
            RpcMessage::new(
                1,
                2,
                MessageType::RequestVoteResponse(RequestVoteResponse {
                    term: 1,
                    vote_granted: true,
                }),
            ),
        );
        assert_eq!(net.pending_count(), 1);
    }

    #[test]
    fn test_rpc_message_serde() {
        let msg = RpcMessage::new(
            1,
            2,
            MessageType::AppendEntriesRequest(AppendEntriesRequest {
                term: 1,
                leader_id: 1,
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![LogEntry {
                    term: 1,
                    index: 1,
                    command: vec![0xAB],
                    config_change: None,
                }],
                leader_commit: 0,
            }),
        );
        let json = serde_json::to_string(&msg).expect("serialize");
        let deserialized: RpcMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn test_raft_state_display() {
        assert_eq!(RaftState::Follower.to_string(), "Follower");
        assert_eq!(RaftState::Candidate.to_string(), "Candidate");
        assert_eq!(RaftState::Leader.to_string(), "Leader");
    }

    #[test]
    fn test_raft_error_display() {
        let err = RaftError::NotLeader(5);
        assert_eq!(err.to_string(), "node 5 is not leader");

        let err = RaftError::InvalidTerm(0);
        assert_eq!(err.to_string(), "invalid term: 0");

        let err = RaftError::LogNotFound(99);
        assert_eq!(err.to_string(), "log entry not found at index 99");

        let err = RaftError::ConfigError("bad peers".to_string());
        assert_eq!(err.to_string(), "config error: bad peers");

        let err = RaftError::InvalidState("bad transition".to_string());
        assert_eq!(err.to_string(), "invalid state transition: bad transition");
    }

    #[test]
    fn test_single_node_becomes_leader() {
        // 单节点集群：majority = 1，自投即可成为 Leader
        let config = Config::single_node();
        let mut node = RaftNode::new(1, config);
        assert_eq!(node.state(), RaftState::Follower);

        node.tick(400); // 选举超时 → Candidate
        assert_eq!(node.state(), RaftState::Candidate);
        assert_eq!(node.current_term(), 1);

        // majority = 1/2 + 1 = 1，自投即满足
        // 但 handle_request_vote_response 需要 from 参数
        // 单节点没有 peers，tick() 不产生消息
        // 单节点的 Candidate 应立即检测 majority（自投=1 >= majority=1）
        // 但当前设计中 become_candidate 不自动检测 majority
        // 单节点应在 tick 中处理
        // 实际上单节点场景：become_candidate 后 votes_received = {self} = 1 >= majority(1)
        // 让我们手动触发
        let msgs = node.handle_request_vote_response(
            1,
            RequestVoteResponse {
                term: 1,
                vote_granted: true,
            },
        );
        assert_eq!(node.state(), RaftState::Leader);
        assert!(msgs.is_empty()); // 无 peers
    }

    // -----------------------------------------------------------------
    //  7. 成员变更接口测试（Phase 8.2 stub，5 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_add_node_leader() {
        let mut leader = RaftNode::new(1, Config::new(vec![2]));
        leader.become_candidate();
        leader.become_leader();

        assert_eq!(leader.peers(), &[2]);
        assert_eq!(leader.cluster_members(), vec![1, 2]);

        leader.add_node(3).expect("add node");
        assert_eq!(leader.peers(), &[2, 3]);
        assert_eq!(leader.cluster_members(), vec![1, 2, 3]);

        // 新节点的 Leader 状态应被初始化
        let ls = leader.leader_state().expect("leader state");
        assert!(ls.next_index.contains_key(&3));
        assert!(ls.match_index.contains_key(&3));
    }

    #[test]
    fn test_add_node_non_leader_returns_error() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        // Follower 状态
        let result = node.add_node(3);
        assert!(matches!(result, Err(RaftError::NotLeader(1))));
        assert_eq!(node.peers(), &[2]); // 配置未变
    }

    #[test]
    fn test_remove_node_leader() {
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3, 4]));
        leader.become_candidate();
        leader.become_leader();

        // 初始 4 节点
        assert_eq!(leader.cluster_members().len(), 4);

        // 移除节点 3
        leader.remove_node(3).expect("remove node");
        assert_eq!(leader.peers(), &[2, 4]);
        assert_eq!(leader.cluster_members(), vec![1, 2, 4]);

        // Leader 状态中对应条目应被清理
        let ls = leader.leader_state().expect("leader state");
        assert!(!ls.next_index.contains_key(&3));
        assert!(!ls.match_index.contains_key(&3));
    }

    #[test]
    fn test_remove_node_non_leader_returns_error() {
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        let result = node.remove_node(2);
        assert!(matches!(result, Err(RaftError::NotLeader(1))));
        // 配置未变
        assert_eq!(node.peers(), &[2, 3]);
    }

    #[test]
    fn test_joint_consensus_interface_exists() {
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        // 写入若干日志以验证 next_index/match_index 重建
        leader.propose(vec![0x01]).expect("propose");
        leader.propose(vec![0x02]).expect("propose");

        // 联合共识：从 {1,2,3} 切换到 {1,2,4}
        let change = MembershipChange::JointConsensus {
            old_peers: vec![1, 2, 3],
            new_peers: vec![1, 2, 4],
        };
        leader
            .propose_membership_change(change)
            .expect("membership");

        // 接口存在且能调用：新配置应为 [1, 2, 4]
        assert_eq!(leader.cluster_members(), vec![1, 2, 4]);
        // 变更状态应为 Completed（stub 直接完成）
        assert_eq!(leader.membership_state(), MembershipChangeState::Completed);
        // Leader 状态针对新 peer 4 已初始化
        let ls = leader.leader_state().expect("leader state");
        assert!(ls.next_index.contains_key(&4));
        // 旧 peer 3 应被移除
        assert!(!ls.next_index.contains_key(&3));
    }

    #[test]
    fn test_membership_change_add_via_propose() {
        let mut leader = RaftNode::new(1, Config::new(vec![2]));
        leader.become_candidate();
        leader.become_leader();

        leader
            .propose_membership_change(MembershipChange::AddNode(3))
            .expect("membership add");
        assert_eq!(leader.peers(), &[2, 3]);
        assert_eq!(leader.membership_state(), MembershipChangeState::Completed);
    }

    #[test]
    fn test_membership_change_remove_via_propose() {
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3, 4]));
        leader.become_candidate();
        leader.become_leader();

        leader
            .propose_membership_change(MembershipChange::RemoveNode(3))
            .expect("membership remove");
        assert_eq!(leader.peers(), &[2, 4]);
        assert_eq!(leader.membership_state(), MembershipChangeState::Completed);
    }

    #[test]
    fn test_membership_change_non_leader_error() {
        let mut node = RaftNode::new(1, Config::new(vec![2]));
        // Follower
        let result = node.propose_membership_change(MembershipChange::AddNode(3));
        assert!(matches!(result, Err(RaftError::NotLeader(1))));
    }

    #[test]
    fn test_add_node_idempotent() {
        let mut leader = RaftNode::new(1, Config::new(vec![2]));
        leader.become_candidate();
        leader.become_leader();

        // 添加自身 → 无操作
        leader.add_node(1).expect("add self");
        assert_eq!(leader.peers(), &[2]);

        // 重复添加已存在节点 → 无操作
        leader.add_node(2).expect("add existing");
        assert_eq!(leader.peers(), &[2]);
    }

    // -----------------------------------------------------------------
    //  8. 故障恢复补充测试（4 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_follower_crash_then_catch_up() {
        // 3 节点：节点 3 初始离线，主集群写入日志后节点 3 上线追赶
        let mut cluster = Cluster::new(&[1, 2, 3], 400);
        cluster.set_offline(3);

        cluster.run_for(500); // 1,2 选举（majority=2，3 离线不影响）
        let leader = cluster.leader().expect("leader elected");

        // 写入 20 条日志
        for i in 0..20u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }
        cluster.run_for(400);

        // 节点 3 上线
        cluster.set_online(3);
        cluster.run_for(800); // 给足时间追赶

        // 节点 3 应已追上
        assert_eq!(
            cluster.get(3).log_len(),
            20,
            "node 3 should catch up to 20 entries, got {}",
            cluster.get(3).log_len()
        );
    }

    #[test]
    fn test_old_leader_steps_down_after_recovery() {
        let mut cluster = Cluster::new(&[1, 2, 3], 500);
        cluster.run_for(500);
        let old_leader = cluster.leader().expect("leader elected");

        // 旧 Leader 离线（模拟崩溃）
        cluster.set_offline(old_leader);
        cluster.run_for(1000); // 剩余节点选出新 Leader

        let new_leader = cluster.leader().expect("new leader");
        assert_ne!(new_leader, old_leader);

        // 旧 Leader 恢复
        cluster.set_online(old_leader);
        cluster.run_for(800);

        // 旧 Leader 应已降级为 Follower（不可能同时有两个 Leader）
        let leaders: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(_, n)| n.state() == RaftState::Leader)
            .map(|(&id, _)| id)
            .collect();
        assert_eq!(leaders.len(), 1, "should have exactly one leader");
        assert_eq!(leaders[0], new_leader);
        // 旧 Leader 的 term 应追赶上新 Leader
        assert!(cluster.get(old_leader).current_term() >= cluster.get(new_leader).current_term());
    }

    #[test]
    fn test_multiple_leader_switches_log_consistency() {
        let mut cluster = Cluster::new(&[1, 2, 3], 600);
        cluster.run_for(500);

        // 多次：写入日志 → 杀 Leader → 新选举 → 写入更多日志
        for round in 0..3u8 {
            let leader = cluster.leader().expect("leader");
            for i in 0..10u8 {
                cluster
                    .get_mut(leader)
                    .propose(vec![round, i])
                    .expect("propose");
            }
            cluster.run_for(400);

            // 杀 Leader
            cluster.set_offline(leader);
            cluster.run_for(1000);

            // 恢复旧 Leader（保证 3 节点在线，避免下轮 majority 不足）
            cluster.set_online(leader);
            cluster.run_for(800);
        }

        // 最终所有在线节点日志应一致
        let mut it = cluster.nodes.values();
        let first = it.next().expect("node");
        let baseline = first.log_entries();
        for (&id, node) in &cluster.nodes {
            assert_eq!(
                node.log_len(),
                baseline.len(),
                "node {} log length mismatch: expected {}, got {}",
                id,
                baseline.len(),
                node.log_len()
            );
            for (i, (a, b)) in baseline.iter().zip(node.log_entries().iter()).enumerate() {
                assert_eq!(a, b, "node {} entry {} mismatch", id, i + 1);
            }
        }
    }

    #[test]
    fn test_three_node_log_fully_identical() {
        let mut cluster = Cluster::new(&[1, 2, 3], 700);
        cluster.run_for(500);
        let leader = cluster.leader().expect("leader");

        // 写入不同命令
        for i in 0..50u32 {
            cluster
                .get_mut(leader)
                .propose(i.to_be_bytes().to_vec())
                .expect("propose");
        }
        cluster.run_for(600);

        // 验证 3 节点日志完全一致（条目数、term、index、command）
        let baseline = cluster.get(leader).log_entries();
        for (&id, node) in &cluster.nodes {
            let entries = node.log_entries();
            assert_eq!(entries.len(), baseline.len(), "node {} length mismatch", id);
            for (i, (a, b)) in baseline.iter().zip(entries.iter()).enumerate() {
                assert_eq!(a, b, "node {} entry {} differs", id, i + 1);
            }
            // commit_index 也应一致
            assert_eq!(
                node.commit_index(),
                cluster.get(leader).commit_index(),
                "node {} commit_index mismatch",
                id
            );
        }
    }

    // -----------------------------------------------------------------
    //  Phase 8.2：Raft 成员变更（联合共识）测试（12 项）
    // -----------------------------------------------------------------

    #[test]
    fn test_v2_propose_returns_error_when_not_leader() {
        // Follower 调用 propose_membership_change_v2 应返回 NotLeader
        let mut node = RaftNode::new(1, Config::new(vec![2, 3]));
        let result = node.propose_membership_change_v2(vec![1, 2, 3, 4]);
        assert!(matches!(result, Err(RaftError::NotLeader(1))));
    }

    #[test]
    fn test_v2_propose_noop_when_config_unchanged() {
        // 新配置与旧配置相同时应无操作
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        let result = leader.propose_membership_change_v2(vec![1, 2, 3]);
        assert!(result.is_ok());
        assert!(leader.joint_consensus().is_none());
        assert_eq!(leader.membership_state(), MembershipChangeState::Stable);
    }

    #[test]
    fn test_v2_propose_rejects_concurrent_change() {
        // 已有变更进行中时再次发起应返回错误
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        // 第一次变更：3→4
        leader
            .propose_membership_change_v2(vec![1, 2, 4])
            .expect("first change");
        // 第二次变更（在第一次未完成时）应失败
        let result = leader.propose_membership_change_v2(vec![1, 2, 3, 4, 5]);
        assert!(matches!(result, Err(RaftError::InvalidState(_))));
    }

    #[test]
    fn test_v2_propose_appends_joint_config_entry() {
        // 发起变更后，日志中应包含 Cold,new 联合配置条目
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        leader
            .propose_membership_change_v2(vec![1, 2, 3, 4])
            .expect("propose");
        let entries = leader.log_entries();
        assert!(!entries.is_empty(), "log should have joint entry");
        let last = entries.last().expect("last entry");
        assert!(
            last.config_change.is_some(),
            "last entry should be a config change"
        );
        let cc = last.config_change.as_ref().expect("config change");
        assert_eq!(cc.stage, ConfigStage::Joint);
        assert_eq!(cc.old_peers, vec![1, 2, 3]);
        assert_eq!(cc.new_peers, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_v2_auto_appends_self_to_new_peers() {
        // new_peers 不含 Leader 自身时，应自动追加
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        leader
            .propose_membership_change_v2(vec![2, 3, 4])
            .expect("propose");
        let last = leader.log_entries().last().expect("last entry");
        let cc = last.config_change.as_ref().expect("config change");
        assert!(cc.new_peers.contains(&1), "Leader should be in Cnew");
    }

    #[test]
    fn test_v2_leader_sets_joint_state() {
        // 发起变更后 Leader 应进入 JointConsensus 状态
        let mut leader = RaftNode::new(1, Config::new(vec![2, 3]));
        leader.become_candidate();
        leader.become_leader();
        leader
            .propose_membership_change_v2(vec![1, 2, 3, 4])
            .expect("propose");
        assert!(leader.joint_consensus().is_some());
        assert_eq!(
            leader.membership_state(),
            MembershipChangeState::JointConsensus
        );
    }

    #[test]
    fn test_v2_config_change_entry_serde() {
        // ConfigChangeEntry 序列化/反序列化往返
        let cc = ConfigChangeEntry::joint(vec![1, 2, 3], vec![1, 2, 3, 4, 5]);
        let json = serde_json::to_string(&cc).expect("serialize");
        let decoded: ConfigChangeEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cc, decoded);
    }

    #[test]
    fn test_v2_new_config_entry_serde() {
        // ConfigStage::New 的序列化
        let cc = ConfigChangeEntry::new_config(vec![1, 2, 3, 4], vec![1, 2, 3]);
        let json = serde_json::to_string(&cc).expect("serialize");
        let decoded: ConfigChangeEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cc, decoded);
        assert_eq!(decoded.stage, ConfigStage::New);
    }

    #[test]
    fn test_v2_5_to_3_remove_two_nodes() {
        // 5 节点 → 移除节点 4 和 5 → 3 节点集群继续正常工作
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 800);
        cluster.run_for(600);
        let leader = cluster.leader().expect("leader elected");

        // 写入若干日志作为基线
        for i in 0..10u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }
        cluster.run_for(400);

        // 先将被移除的节点 4、5 设为离线，避免它们在变更过程中启动选举干扰
        // （Cold 多数派 = 3/5，1、2、3 刚好满足；Cnew 多数派 = 2/3 也满足）
        cluster.set_offline(4);
        cluster.set_offline(5);

        // 发起成员变更：5→3（移除 4、5）
        cluster
            .get_mut(leader)
            .propose_membership_change_v2(vec![1, 2, 3])
            .expect("propose v2");

        // 运行足够时间让两阶段提交完成
        cluster.run_for(1500);

        // 验证：所有存活节点（1、2、3）的 config.peers 应只含 [2,3] / [1,3] / [1,2]
        for &id in &[1, 2, 3] {
            let node = cluster.get(id);
            let members = node.cluster_members();
            assert_eq!(
                members,
                vec![1, 2, 3],
                "node {} should be in 3-node config, got {:?}",
                id,
                members
            );
            assert!(
                node.joint_consensus().is_none(),
                "node {} should have completed membership change",
                id
            );
        }

        // 集群继续工作：写入更多日志
        for i in 10..20u8 {
            let leader = cluster.leader().expect("leader");
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
            cluster.run_for(200);
        }

        // 3 节点日志应完全一致（20 条 + 配置变更条目）
        let baseline = cluster.get(1).log_len();
        for &id in &[2, 3] {
            assert_eq!(
                cluster.get(id).log_len(),
                baseline,
                "node {} log length mismatch after membership change",
                id
            );
        }

        // 验证只有一个 Leader
        let leaders: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(&id, _)| !cluster.network.is_offline(id))
            .filter(|(_, n)| n.state() == RaftState::Leader)
            .map(|(&id, _)| id)
            .collect();
        assert_eq!(leaders.len(), 1, "should have exactly one leader");
    }

    #[test]
    fn test_v2_3_to_5_add_two_nodes() {
        // 3 节点 → 加入节点 4 和 5 → 5 节点自动日志同步
        let mut cluster = Cluster::new(&[1, 2, 3], 900);
        cluster.run_for(600);
        let leader = cluster.leader().expect("leader elected");

        // 写入基线日志
        for i in 0..5u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }
        cluster.run_for(300);

        // 加入两个新节点（尚未在集群配置中）
        for &new_id in &[4, 5] {
            let config = Config {
                peers: [1, 2, 3].iter().copied().filter(|&p| p != new_id).collect(),
                election_timeout_min_ms: 150,
                election_timeout_max_ms: 300,
                heartbeat_interval_ms: 50,
                seed: 900,
            };
            cluster.nodes.insert(new_id, RaftNode::new(new_id, config));
        }

        // 发起成员变更：3→5
        let leader = cluster.leader().expect("leader");
        cluster
            .get_mut(leader)
            .propose_membership_change_v2(vec![1, 2, 3, 4, 5])
            .expect("propose v2");

        // 运行足够时间让两阶段提交完成 + 新节点追赶
        cluster.run_for(2000);

        // 验证：所有 5 节点应在新配置中
        for &id in &[1, 2, 3, 4, 5] {
            let node = cluster.get(id);
            let members = node.cluster_members();
            assert_eq!(
                members,
                vec![1, 2, 3, 4, 5],
                "node {} should be in 5-node config, got {:?}",
                id,
                members
            );
        }

        // 新节点 4、5 应已追上基线日志（5 条数据 + 配置变更条目）
        let baseline = cluster.get(1).log_len();
        for &id in &[2, 3, 4, 5] {
            assert_eq!(
                cluster.get(id).log_len(),
                baseline,
                "node {} should have full log, got {} vs {}",
                id,
                cluster.get(id).log_len(),
                baseline
            );
        }

        // 验证只有一个 Leader
        let leaders: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(_, n)| n.state() == RaftState::Leader)
            .map(|(&id, _)| id)
            .collect();
        assert_eq!(leaders.len(), 1, "should have exactly one leader");
    }

    #[test]
    fn test_v2_5_to_3_to_5_full_cycle() {
        // 完整循环：5 节点 → 移除 2 → 3 节点 → 加回 2 → 5 节点
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 1000);
        cluster.run_for(600);
        let leader = cluster.leader().expect("leader");

        // 阶段 1：写入基线日志
        for i in 0..5u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }
        cluster.run_for(400);

        // 阶段 2：5→3 移除 4、5
        // 先将被移除的节点 4、5 设为离线，避免选举干扰
        cluster.set_offline(4);
        cluster.set_offline(5);
        let leader = cluster.leader().expect("leader");
        cluster
            .get_mut(leader)
            .propose_membership_change_v2(vec![1, 2, 3])
            .expect("5→3");
        cluster.run_for(1500);

        // 验证 3 节点配置
        for &id in &[1, 2, 3] {
            assert_eq!(cluster.get(id).cluster_members(), vec![1, 2, 3]);
        }
        let log_len_after_shrink = cluster.get(1).log_len();

        // 阶段 3：3→5 加回 4、5
        // 用全新实例替换旧的节点 4、5（旧实例状态已不一致）
        cluster.nodes.remove(&4);
        cluster.nodes.remove(&5);
        cluster.set_online(4);
        cluster.set_online(5);
        for &new_id in &[4, 5] {
            let config = Config {
                peers: vec![1, 2, 3],
                election_timeout_min_ms: 150,
                election_timeout_max_ms: 300,
                heartbeat_interval_ms: 50,
                seed: 1000,
            };
            cluster.nodes.insert(new_id, RaftNode::new(new_id, config));
        }
        let leader = cluster.leader().expect("leader");
        cluster
            .get_mut(leader)
            .propose_membership_change_v2(vec![1, 2, 3, 4, 5])
            .expect("3→5");
        cluster.run_for(2000);

        // 验证 5 节点配置
        for &id in &[1, 2, 3, 4, 5] {
            assert_eq!(
                cluster.get(id).cluster_members(),
                vec![1, 2, 3, 4, 5],
                "node {} should be in 5-node config",
                id
            );
        }

        // 新节点 4、5 应已追上日志（3→5 变更追加 Cold,new + Cnew 共 2 条配置变更条目）
        for &id in &[4, 5] {
            assert_eq!(
                cluster.get(id).log_len(),
                log_len_after_shrink + 2,
                "node {} should have caught up",
                id
            );
        }

        // 阶段 4：变更后继续写入日志，验证集群正常工作
        let leader = cluster.leader().expect("leader");
        for i in 5..15u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }
        cluster.run_for(600);

        // 所有 5 节点日志应一致
        let baseline = cluster.get(1).log_len();
        for &id in &[2, 3, 4, 5] {
            assert_eq!(
                cluster.get(id).log_len(),
                baseline,
                "node {} log mismatch after full cycle",
                id
            );
        }
    }

    #[test]
    fn test_v2_no_data_loss_during_membership_change() {
        // 成员变更期间不丢失日志
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 1100);
        cluster.run_for(600);
        let leader = cluster.leader().expect("leader");

        // 写入 30 条日志
        for i in 0..30u8 {
            cluster.get_mut(leader).propose(vec![i]).expect("propose");
        }
        cluster.run_for(500);

        let log_len_before = cluster.get(1).log_len();

        // 发起成员变更：5→3
        let leader = cluster.leader().expect("leader");
        cluster
            .get_mut(leader)
            .propose_membership_change_v2(vec![1, 2, 3])
            .expect("5→3");
        cluster.run_for(1500);

        // 变更后 3 节点日志应 >= 变更前（不丢失）
        for &id in &[1, 2, 3] {
            assert!(
                cluster.get(id).log_len() >= log_len_before,
                "node {} lost log entries: before={}, after={}",
                id,
                log_len_before,
                cluster.get(id).log_len()
            );
        }
    }

    #[test]
    fn test_v2_no_split_brain_during_membership_change() {
        // 成员变更期间不出现双主
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 1200);
        cluster.run_for(600);

        let leader = cluster.leader().expect("leader");
        cluster
            .get_mut(leader)
            .propose_membership_change_v2(vec![1, 2, 3])
            .expect("5→3");

        // 变更过程中持续检查只有一个 Leader
        for _ in 0..20 {
            cluster.run_for(100);
            let leaders: Vec<NodeId> = cluster
                .nodes
                .iter()
                .filter(|(&id, _)| !cluster.network.is_offline(id))
                .filter(|(_, n)| n.state() == RaftState::Leader)
                .map(|(&id, _)| id)
                .collect();
            assert!(
                leaders.len() <= 1,
                "split brain detected: {} leaders {:?}",
                leaders.len(),
                leaders
            );
        }
    }

    // -----------------------------------------------------------------
    //  Phase 8.3：Raft 故障 fuzz 测试（8 项）
    //
    //  使用确定性 LCG 驱动随机故障注入，保证测试可复现。
    //  故障类型：节点 kill/重启、网络分区/恢复、随机延迟。
    // -----------------------------------------------------------------

    /// Fuzz 测试辅助：断言所有在线节点日志一致（或都为空）
    fn assert_online_logs_consistent(cluster: &Cluster) {
        let online_nodes: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(&id, _)| !cluster.network.is_offline(id))
            .map(|(&id, _)| id)
            .collect();
        if online_nodes.len() < 2 {
            return;
        }
        let baseline = cluster.get(online_nodes[0]).log_len();
        for &id in &online_nodes[1..] {
            assert_eq!(
                cluster.get(id).log_len(),
                baseline,
                "fuzz: node {} log len {} != baseline {} (node {})",
                id,
                cluster.get(id).log_len(),
                baseline,
                online_nodes[0]
            );
        }
    }

    /// Fuzz 测试辅助：断言在线节点中最多一个 Leader
    fn assert_at_most_one_leader(cluster: &Cluster) {
        let leaders: Vec<NodeId> = cluster
            .nodes
            .iter()
            .filter(|(&id, _)| !cluster.network.is_offline(id))
            .filter(|(_, n)| n.state() == RaftState::Leader)
            .map(|(&id, _)| id)
            .collect();
        assert!(
            leaders.len() <= 1,
            "fuzz: split brain {} leaders {:?}",
            leaders.len(),
            leaders
        );
    }

    #[test]
    fn test_fuzz_random_kill_restart_50_rounds() {
        // 随机 kill/重启节点 50 轮 → 验证日志一致性和 Leader 唯一性
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5000);
        let mut rng = Lcg::new(5000);

        // 初始选举
        cluster.run_for(600);

        // 持续写入 + 随机故障
        for round in 0..50u32 {
            // 写入若干日志
            if let Some(leader) = cluster.leader() {
                for i in 0..5u8 {
                    cluster
                        .get_mut(leader)
                        .propose(vec![round as u8, i])
                        .expect("propose");
                }
                cluster.run_for(200);
            }

            // 随机故障：kill 一个节点
            let victim = (rng.next_u32() % 5 + 1) as NodeId;
            cluster.set_offline(victim);

            cluster.run_for(300);

            // 随机恢复：重启被 kill 的节点
            cluster.set_online(victim);

            cluster.run_for(500);

            // 每轮检查
            assert_at_most_one_leader(&cluster);
        }

        // 最终运行足够时间让集群稳定
        cluster.run_for(1000);

        // 验证：所有在线节点日志一致 + 单 Leader
        assert_online_logs_consistent(&cluster);
        assert_at_most_one_leader(&cluster);
    }

    #[test]
    fn test_fuzz_random_network_partition_50_rounds() {
        // 随机网络分区/恢复 50 轮 → 验证日志一致性和 Leader 唯一性
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5100);
        let mut rng = Lcg::new(5100);

        cluster.run_for(600);

        for round in 0..50u32 {
            if let Some(leader) = cluster.leader() {
                for i in 0..3u8 {
                    cluster
                        .get_mut(leader)
                        .propose(vec![round as u8, i])
                        .expect("propose");
                }
                cluster.run_for(200);
            }

            // 随机分区：隔离一对节点
            let a = (rng.next_u32() % 5 + 1) as NodeId;
            let b = (rng.next_u32() % 5 + 1) as NodeId;
            if a != b {
                cluster.partition(a, b);
            }

            cluster.run_for(300);

            // 恢复所有链路
            cluster.heal_all();

            cluster.run_for(500);

            assert_at_most_one_leader(&cluster);
        }

        cluster.run_for(1000);
        assert_online_logs_consistent(&cluster);
        assert_at_most_one_leader(&cluster);
    }

    #[test]
    fn test_fuzz_combined_kill_and_partition_50_rounds() {
        // 组合故障：随机 kill + 随机分区 50 轮
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5200);
        let mut rng = Lcg::new(5200);

        cluster.run_for(600);

        for round in 0..50u32 {
            if let Some(leader) = cluster.leader() {
                cluster
                    .get_mut(leader)
                    .propose(vec![round as u8])
                    .expect("propose");
                cluster.run_for(200);
            }

            // 50% 概率 kill 节点
            if rng.next_u32().is_multiple_of(2) {
                let victim = (rng.next_u32() % 5 + 1) as NodeId;
                cluster.set_offline(victim);
            }

            // 50% 概率分区
            if rng.next_u32().is_multiple_of(2) {
                let a = (rng.next_u32() % 5 + 1) as NodeId;
                let b = (rng.next_u32() % 5 + 1) as NodeId;
                if a != b {
                    cluster.partition(a, b);
                }
            }

            cluster.run_for(300);

            // 恢复所有故障
            cluster.heal_all();
            for &id in &[1, 2, 3, 4, 5] {
                cluster.set_online(id);
            }

            cluster.run_for(500);

            assert_at_most_one_leader(&cluster);
        }

        cluster.run_for(1000);
        assert_online_logs_consistent(&cluster);
        assert_at_most_one_leader(&cluster);
    }

    #[test]
    fn test_fuzz_leader_kill_50_rounds() {
        // 专门 kill Leader 50 轮 → 验证每次都能选出新 Leader 且日志不丢失
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5300);

        cluster.run_for(600);

        for round in 0..50u32 {
            let leader = cluster.leader().expect("leader should exist");

            // 写入日志
            for i in 0..3u8 {
                cluster
                    .get_mut(leader)
                    .propose(vec![round as u8, i])
                    .expect("propose");
            }
            cluster.run_for(200);

            // Kill Leader
            cluster.set_offline(leader);
            cluster.run_for(1000);

            // 恢复旧 Leader
            cluster.set_online(leader);
            cluster.run_for(800);

            assert_at_most_one_leader(&cluster);
        }

        cluster.run_for(1000);

        // 验证：所有在线节点日志一致 + 单 Leader
        assert_online_logs_consistent(&cluster);
        assert_at_most_one_leader(&cluster);

        // 日志不应为空（至少有部分已提交的日志）
        let any_node = cluster
            .nodes
            .iter()
            .filter(|(_, n)| !n.log_entries().is_empty())
            .count();
        assert!(any_node > 0, "should have some committed log entries");
    }

    #[test]
    fn test_fuzz_minority_partition_50_rounds() {
        // 少数派分区（隔离 1-2 个节点）50 轮 → 多数派应继续工作
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5400);
        let mut rng = Lcg::new(5400);

        cluster.run_for(600);

        for round in 0..50u32 {
            if let Some(leader) = cluster.leader() {
                cluster
                    .get_mut(leader)
                    .propose(vec![round as u8])
                    .expect("propose");
                cluster.run_for(200);
            }

            // 隔离 1 个节点（少数派分区）
            let isolated = (rng.next_u32() % 5 + 1) as NodeId;
            cluster.set_offline(isolated);

            cluster.run_for(300);

            // 恢复
            cluster.set_online(isolated);
            cluster.run_for(500);

            assert_at_most_one_leader(&cluster);
        }

        cluster.run_for(1000);
        assert_online_logs_consistent(&cluster);
        assert_at_most_one_leader(&cluster);
    }

    #[test]
    fn test_fuzz_log_consistency_after_50_failures() {
        // 50 次故障后所有存活节点日志一致（进度表验证标准）
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5500);
        let mut rng = Lcg::new(5500);

        cluster.run_for(600);

        // 写入基线日志
        if let Some(leader) = cluster.leader() {
            for i in 0..20u8 {
                cluster.get_mut(leader).propose(vec![i]).expect("propose");
            }
            cluster.run_for(500);
        }

        let baseline_len = cluster.get(1).log_len();

        // 50 次随机故障
        for _ in 0..50u32 {
            // 随机 kill
            let victim = (rng.next_u32() % 5 + 1) as NodeId;
            cluster.set_offline(victim);
            cluster.run_for(200);

            // 随机恢复
            cluster.set_online(victim);
            cluster.run_for(400);
        }

        cluster.run_for(1000);

        // 验证：所有在线节点日志一致
        assert_online_logs_consistent(&cluster);

        // 日志长度应 >= 基线（不丢失已提交日志）
        for &id in &[1, 2, 3, 4, 5] {
            if !cluster.network.is_offline(id) {
                assert!(
                    cluster.get(id).log_len() >= baseline_len,
                    "node {} lost committed logs: baseline={}, current={}",
                    id,
                    baseline_len,
                    cluster.get(id).log_len()
                );
            }
        }
    }

    #[test]
    fn test_fuzz_no_data_loss_under_chaos() {
        // 混沌环境：同时 kill + 分区 + 写入 → 验证不丢失已提交数据
        let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], 5600);
        let mut rng = Lcg::new(5600);

        cluster.run_for(600);

        // 记录已提交的 commit_index
        let mut max_commit = 0u64;

        for round in 0..30u32 {
            // 写入
            if let Some(leader) = cluster.leader() {
                cluster
                    .get_mut(leader)
                    .propose(vec![round as u8])
                    .expect("propose");
                cluster.run_for(300);

                // 记录最大 commit_index
                let ci = cluster.get(leader).commit_index();
                if ci > max_commit {
                    max_commit = ci;
                }
            }

            // 随机故障
            let victim = (rng.next_u32() % 5 + 1) as NodeId;
            cluster.set_offline(victim);
            cluster.run_for(200);

            // 恢复
            cluster.set_online(victim);
            cluster.run_for(500);
        }

        cluster.run_for(1000);

        // 验证：所有在线节点的 commit_index 应 >= max_commit（已提交的不丢失）
        for (&id, node) in &cluster.nodes {
            if !cluster.network.is_offline(id) {
                assert!(
                    node.commit_index() >= max_commit,
                    "node {} lost committed data: max_commit={}, node_commit={}",
                    id,
                    max_commit,
                    node.commit_index()
                );
            }
        }
    }

    #[test]
    fn test_fuzz_deterministic_reproducible() {
        // 相同种子应产生相同的故障序列和结果（可复现性验证）
        fn run_fuzz(seed: u64) -> usize {
            let mut cluster = Cluster::new(&[1, 2, 3, 4, 5], seed);
            let mut rng = Lcg::new(seed);

            cluster.run_for(600);

            for _ in 0..20u32 {
                if let Some(leader) = cluster.leader() {
                    cluster
                        .get_mut(leader)
                        .propose(vec![0xAA])
                        .expect("propose");
                    cluster.run_for(200);
                }
                let victim = (rng.next_u32() % 5 + 1) as NodeId;
                cluster.set_offline(victim);
                cluster.run_for(200);
                cluster.set_online(victim);
                cluster.run_for(400);
            }

            cluster.run_for(500);
            cluster.get(1).log_len()
        }

        let len1 = run_fuzz(5700);
        let len2 = run_fuzz(5700);
        assert_eq!(
            len1, len2,
            "same seed should produce same result: {} vs {}",
            len1, len2
        );
    }
}
