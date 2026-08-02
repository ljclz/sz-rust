//! 分布式 CDC 协调模块 — 对应 `SzRSQL实施进度.md` P6-1。
//!
//! 在单机 `ReplicationTaskManager`（`task.rs`）之上，提供多节点 CDC 任务分配、
//! 负载均衡、故障转移能力。
//!
//! # 核心概念
//!
//! - **ClusterNode**：集群节点抽象，维护节点容量、状态、角色
//! - **ClusterCoordinator**：集群协调器，维护节点列表 + 任务分配映射
//! - **TaskAssignment**：任务分配策略（基于容量负载均衡 + 任务亲和性）
//! - **HeartbeatProvider / TaskDispatcher**：闭包注入 trait，生产部署时由调用方注入
//!
//! # 设计要点
//!
//! 1. **闭包注入模式**：与 `source/`、`target/` 一致，避免直接依赖网络库
//!    - `HeartbeatProvider` trait：心跳网络通信（生产部署时注入）
//!    - `TaskDispatcher` trait：任务分发网络通信（生产部署时注入）
//! 2. **同步接口**：与 `TargetWriter`/`SourceConnector` 一致
//! 3. **线程安全**：`RwLock + Arc`，支持并发注册/分配/查询
//! 4. **状态机**：`NodeStatus` 转换（Alive → Dead → 离开集群）
//! 5. **任务亲和性**：同一 `source_id` 的任务分配到同节点，减少状态同步开销
//! 6. **负载均衡**：选择 `current_tasks < max_tasks` 且 `cpu_usage` 最低的节点
//!
//! # 与 szrsql-dist 的关系（L11 修复说明）
//!
//! szrsql-dist 的 `TcpNetwork` 是底层通用网络传输层（关注 TCP 包收发、连接管理），
//! 服务于 Raft 共识、Percolator 2PC 等分布式基础原语。
//!
//! 本模块的 `ClusterCoordinator` 是上层 CDC 任务调度协调器（关注任务分配、
//! 负载均衡、亲和性、故障迁移），通过 `HeartbeatProvider`/`TaskDispatcher`
//! trait 抽象网络通信。
//!
//! 两者**职责不同，非重复实现**：
//! - szrsql-dist/network.rs：通用网络层（传输字节流）
//! - szrsql-cdc/cluster.rs：CDC 任务调度层（分配任务到节点）
//!
//! 生产部署时，调用方应实现 `HeartbeatProvider`/`TaskDispatcher` trait，
//! 内部委托给 szrsql-dist 的 `TcpNetwork` 进行实际网络通信，
//! 形成"CDC 调度层 → HeartbeatProvider 适配器 → TcpNetwork 传输层"的清晰分层。
//!
//! # 状态转换图
//!
//! ```text
//!    register ──▶ Alive ──heartbeat_timeout──▶ Dead ──unregister──▶ 离开集群
//!                   │                            │
//!                   │                            └──enable_migration──▶ 任务迁移到其他 Alive 节点
//!                   └──unregister──▶ 离开集群（任务先迁移）
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================
// ClusterError — 集群错误
// =====================================================================

/// 集群协调错误
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    /// 节点已存在
    #[error("node already exists: {0}")]
    NodeAlreadyExists(String),

    /// 节点不存在
    #[error("node not found: {0}")]
    NodeNotFound(String),

    /// 节点已死亡
    #[error("node is dead: {0}")]
    NodeDead(String),

    /// 节点正在离开
    #[error("node is leaving: {0}")]
    NodeLeaving(String),

    /// 任务已分配
    #[error("task already assigned: {task_id} on node {node_id}")]
    TaskAlreadyAssigned { task_id: String, node_id: String },

    /// 任务未分配
    #[error("task not assigned: {0}")]
    TaskNotAssigned(String),

    /// 无可用节点（所有节点已满或无 Alive 节点）
    #[error("no available node: {0}")]
    NoAvailableNode(String),

    /// 节点容量已满
    #[error("node capacity full: {0}")]
    NodeCapacityFull(String),

    /// 配置错误
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// 心跳提供者错误
    #[error("heartbeat provider error: {0}")]
    HeartbeatProvider(String),

    /// 任务分发器错误
    #[error("task dispatcher error: {0}")]
    TaskDispatcher(String),

    /// 内部错误
    #[error("internal error: {0}")]
    Internal(String),
}

// =====================================================================
// ClusterConfig — 集群配置
// =====================================================================

/// 集群配置 — 创建 `ClusterCoordinator` 时提供
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// 心跳发送间隔（毫秒，默认 10000）
    pub heartbeat_interval_ms: u64,
    /// 心跳超时阈值（毫秒，默认 30000）
    ///
    /// 超过此时间未收到节点心跳，标记为 Dead
    pub heartbeat_timeout_ms: u64,
    /// 单节点最大任务数（默认 10）
    pub max_tasks_per_node: u32,
    /// 是否启用任务自动迁移（节点 Dead 时迁移其任务，默认 true）
    pub enable_task_migration: bool,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_ms: 10_000,
            heartbeat_timeout_ms: 30_000,
            max_tasks_per_node: 10,
            enable_task_migration: true,
        }
    }
}

impl ClusterConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置心跳间隔
    pub fn with_heartbeat_interval_ms(mut self, ms: u64) -> Self {
        self.heartbeat_interval_ms = ms;
        self
    }

    /// 设置心跳超时
    pub fn with_heartbeat_timeout_ms(mut self, ms: u64) -> Self {
        self.heartbeat_timeout_ms = ms;
        self
    }

    /// 设置单节点最大任务数
    pub fn with_max_tasks_per_node(mut self, max: u32) -> Self {
        self.max_tasks_per_node = max;
        self
    }

    /// 启用/禁用任务迁移
    pub fn with_task_migration(mut self, enable: bool) -> Self {
        self.enable_task_migration = enable;
        self
    }

    /// 校验配置合法性
    pub fn validate(&self) -> Result<(), ClusterError> {
        if self.heartbeat_interval_ms == 0 {
            return Err(ClusterError::InvalidConfig(
                "heartbeat_interval_ms must be > 0".to_string(),
            ));
        }
        if self.heartbeat_timeout_ms < self.heartbeat_interval_ms {
            return Err(ClusterError::InvalidConfig(
                "heartbeat_timeout_ms must be >= heartbeat_interval_ms".to_string(),
            ));
        }
        if self.max_tasks_per_node == 0 {
            return Err(ClusterError::InvalidConfig(
                "max_tasks_per_node must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

// =====================================================================
// NodeRole / NodeStatus — 节点角色与状态
// =====================================================================

/// 节点角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    /// 主节点（任务分配决策者）
    Leader,
    /// 从节点（任务执行者）
    Follower,
    /// 候选节点（选举中）
    Candidate,
}

impl NodeRole {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            NodeRole::Leader => "leader",
            NodeRole::Follower => "follower",
            NodeRole::Candidate => "candidate",
        }
    }
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 节点状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// 存活（正常工作）
    Alive,
    /// 死亡（心跳超时）
    Dead,
    /// 正在离开（等待任务迁移完成）
    Leaving,
}

impl NodeStatus {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Alive => "alive",
            NodeStatus::Dead => "dead",
            NodeStatus::Leaving => "leaving",
        }
    }

    /// 是否可接受新任务
    pub fn can_accept_tasks(self) -> bool {
        matches!(self, Self::Alive)
    }

    /// 是否终态（可清理）
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Dead)
    }
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// NodeCapacity — 节点容量
// =====================================================================

/// 节点容量 — 描述节点资源使用情况
///
/// **字段**：
/// - `max_tasks`：最大任务数上限
/// - `current_tasks`：当前已分配任务数
/// - `cpu_usage`：CPU 使用率（0-100，百分比）
/// - `memory_usage`：内存使用率（0-100，百分比）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeCapacity {
    /// 最大任务数上限
    pub max_tasks: u32,
    /// 当前已分配任务数
    pub current_tasks: u32,
    /// CPU 使用率（0-100）
    pub cpu_usage: f64,
    /// 内存使用率（0-100）
    pub memory_usage: f64,
}

impl NodeCapacity {
    /// 创建容量描述
    pub fn new(max_tasks: u32) -> Self {
        Self {
            max_tasks,
            current_tasks: 0,
            cpu_usage: 0.0,
            memory_usage: 0.0,
        }
    }

    /// 是否还有剩余容量
    pub fn has_capacity(&self) -> bool {
        self.current_tasks < self.max_tasks
    }

    /// 剩余可用任务数
    pub fn available_capacity(&self) -> u32 {
        self.max_tasks.saturating_sub(self.current_tasks)
    }

    /// 负载分数（0.0-1.0，越低越空闲）
    pub fn load_score(&self) -> f64 {
        if self.max_tasks == 0 {
            return 1.0;
        }
        // 综合 current_tasks / max_tasks 与 cpu_usage
        let task_load = self.current_tasks as f64 / self.max_tasks as f64;
        let cpu_load = self.cpu_usage / 100.0;
        // 加权平均（任务负载 0.6 + CPU 0.4）
        0.6 * task_load + 0.4 * cpu_load
    }
}

impl Default for NodeCapacity {
    fn default() -> Self {
        Self::new(10)
    }
}

// =====================================================================
// ClusterNode — 集群节点
// =====================================================================

/// 集群节点 — 描述一个 CDC 工作节点
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClusterNode {
    /// 节点 ID（唯一标识）
    pub node_id: String,
    /// 节点地址（host:port）
    pub address: String,
    /// 节点角色
    pub role: NodeRole,
    /// 节点容量
    pub capacity: NodeCapacity,
    /// 节点状态
    pub status: NodeStatus,
    /// 最后一次心跳时间戳（Unix 毫秒）
    pub last_heartbeat: u64,
    /// 注册时间戳（Unix 毫秒）
    pub registered_at: u64,
}

impl ClusterNode {
    /// 创建新节点（状态 Alive，角色 Follower）
    pub fn new(node_id: impl Into<String>, address: impl Into<String>, max_tasks: u32) -> Self {
        let now = current_millis();
        Self {
            node_id: node_id.into(),
            address: address.into(),
            role: NodeRole::Follower,
            capacity: NodeCapacity::new(max_tasks),
            status: NodeStatus::Alive,
            last_heartbeat: now,
            registered_at: now,
        }
    }

    /// 是否 Alive
    pub fn is_alive(&self) -> bool {
        self.status == NodeStatus::Alive
    }

    /// 是否 Dead
    pub fn is_dead(&self) -> bool {
        self.status == NodeStatus::Dead
    }

    /// 是否可接受新任务
    pub fn can_accept_tasks(&self) -> bool {
        self.is_alive() && self.capacity.has_capacity()
    }

    /// 更新心跳
    pub fn touch_heartbeat(&mut self, now: u64) {
        self.last_heartbeat = now;
    }

    /// 检查心跳是否超时
    pub fn is_heartbeat_timeout(&self, now: u64, timeout_ms: u64) -> bool {
        // last_heartbeat == 0 表示从未收到心跳，不算超时（注册时已设为当前时间）
        now.saturating_sub(self.last_heartbeat) > timeout_ms
    }
}

// =====================================================================
// TaskAssignment — 任务分配策略
// =====================================================================

/// 任务分配策略 — 基于容量负载均衡 + 任务亲和性
///
/// **算法**：
/// 1. **亲和性优先**：若指定 `source_id`，且已有节点持有同源任务，优先分配到该节点
///    （减少状态同步开销，复用 source 连接/schema 缓存）
/// 2. **负载均衡**：从 Alive 且有剩余容量的节点中选择 `load_score` 最低的
/// 3. **CPU 优先**：负载分数相同时，选择 `cpu_usage` 最低的
/// 4. **节点 ID 稳定**：分数完全相同时，按 node_id 字典序选择最小的（保证幂等）
pub struct TaskAssignment;

impl TaskAssignment {
    /// 选择最佳节点
    ///
    /// # 参数
    /// - `nodes`：所有候选节点（仅 Alive 且有容量）
    /// - `affinity_node_id`：亲和性节点 ID（若 Some，优先选择该节点）
    ///
    /// # 返回
    /// - `Some(node_id)`：最佳节点 ID
    /// - `None`：无可用节点
    pub fn select_node<'a>(
        nodes: &'a [ClusterNode],
        affinity_node_id: Option<&str>,
    ) -> Option<&'a ClusterNode> {
        // 亲和性优先：若指定亲和节点且该节点在候选列表中，直接返回
        if let Some(aff_id) = affinity_node_id {
            if let Some(node) = nodes.iter().find(|n| n.node_id == aff_id) {
                return Some(node);
            }
        }

        // 负载均衡：选 load_score 最低的，相同时 cpu_usage 最低，再相同时 node_id 字典序最小
        nodes.iter().min_by(|a, b| {
            a.capacity
                .load_score()
                .partial_cmp(&b.capacity.load_score())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.capacity
                        .cpu_usage
                        .partial_cmp(&b.capacity.cpu_usage)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.node_id.cmp(&b.node_id))
        })
    }

    /// 过滤出可接受任务的节点（Alive 且有剩余容量）
    pub fn filter_available(nodes: &[ClusterNode]) -> Vec<&ClusterNode> {
        nodes.iter().filter(|n| n.can_accept_tasks()).collect()
    }
}

// =====================================================================
// HeartbeatProvider / TaskDispatcher — 闭包注入 trait
// =====================================================================

/// 心跳提供者 — 抽象节点间心跳通信
///
/// **实现者责任**：
/// 1. `send_heartbeat`：向指定节点发送心跳
/// 2. `check_node_alive`：检查节点是否存活（主动探测）
///
/// **生产部署**：由调用方注入基于 TCP/gRPC 的实际网络通信实现
/// **测试**：可注入内存实现或返回 Ok(()) 的 no-op 实现
pub trait HeartbeatProvider: Send + Sync {
    /// 向节点发送心跳
    fn send_heartbeat(&self, node: &ClusterNode) -> Result<(), ClusterError>;

    /// 主动探测节点是否存活
    fn check_node_alive(&self, node: &ClusterNode) -> Result<bool, ClusterError>;
}

/// 任务分发器 — 抽象任务下发通信
///
/// **实现者责任**：
/// 1. `dispatch_task`：将任务分发到指定节点
/// 2. `undispatch_task`：通知节点取消任务
/// 3. `migrate_task`：通知源节点停止任务、目标节点启动任务
///
/// **生产部署**：由调用方注入基于 TCP/gRPC 的实际网络通信实现
pub trait TaskDispatcher: Send + Sync {
    /// 将任务分发到节点
    fn dispatch_task(
        &self,
        task_id: &str,
        source_id: &str,
        target_node: &ClusterNode,
    ) -> Result<(), ClusterError>;

    /// 通知节点取消任务
    fn undispatch_task(&self, task_id: &str, node: &ClusterNode) -> Result<(), ClusterError>;

    /// 迁移任务（源节点 → 目标节点）
    fn migrate_task(
        &self,
        task_id: &str,
        source_node: &ClusterNode,
        target_node: &ClusterNode,
    ) -> Result<(), ClusterError>;
}

// =====================================================================
// NoopHeartbeatProvider / NoopTaskDispatcher — 默认 no-op 实现（测试用）
// =====================================================================

/// 默认心跳提供者（no-op，所有操作返回 Ok）
#[derive(Debug, Default, Clone)]
pub struct NoopHeartbeatProvider;

impl HeartbeatProvider for NoopHeartbeatProvider {
    fn send_heartbeat(&self, _node: &ClusterNode) -> Result<(), ClusterError> {
        Ok(())
    }

    fn check_node_alive(&self, node: &ClusterNode) -> Result<bool, ClusterError> {
        Ok(node.is_alive())
    }
}

/// 默认任务分发器（no-op，所有操作返回 Ok）
#[derive(Debug, Default, Clone)]
pub struct NoopTaskDispatcher;

impl TaskDispatcher for NoopTaskDispatcher {
    fn dispatch_task(
        &self,
        _task_id: &str,
        _source_id: &str,
        _target_node: &ClusterNode,
    ) -> Result<(), ClusterError> {
        Ok(())
    }

    fn undispatch_task(&self, _task_id: &str, _node: &ClusterNode) -> Result<(), ClusterError> {
        Ok(())
    }

    fn migrate_task(
        &self,
        _task_id: &str,
        _source_node: &ClusterNode,
        _target_node: &ClusterNode,
    ) -> Result<(), ClusterError> {
        Ok(())
    }
}

// =====================================================================
// ClusterCoordinator — 集群协调器
// =====================================================================

/// 集群协调器 — 维护节点列表 + 任务分配映射，提供分布式 CDC 任务协调
///
/// **内部结构**：
/// - `nodes`：节点列表（`RwLock<HashMap<String, ClusterNode>>`）
/// - `assignments`：任务分配映射（`task_id -> node_id`）
/// - `task_sources`：任务到 source 的映射（`task_id -> source_id`，用于亲和性）
/// - `source_nodes`：source 到节点的映射（`source_id -> node_id`，亲和性索引）
/// - `config`：集群配置
/// - `heartbeat_provider` / `task_dispatcher`：可注入的网络通信提供者
/// - `timestamp_fn`：时间戳注入函数（便于测试固定时间戳）
///
/// **线程安全**：所有状态用 `RwLock` 保护，支持并发读、互斥写
///
/// **使用示例**：
/// ```ignore
/// use szrsql_cdc::cluster::{ClusterCoordinator, ClusterConfig};
///
/// let coord = ClusterCoordinator::new(ClusterConfig::default());
/// coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
/// coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
///
/// // 分配任务（带 source_id 实现亲和性）
/// let node = coord.assign_task("task-1", "pg-source-1").unwrap();
/// println!("task-1 assigned to {}", node);
///
/// // 同源任务分配到同节点
/// let node2 = coord.assign_task("task-2", "pg-source-1").unwrap();
/// assert_eq!(node, node2);
/// ```
pub struct ClusterCoordinator {
    /// 节点列表 node_id -> ClusterNode
    nodes: RwLock<HashMap<String, ClusterNode>>,
    /// 任务分配映射 task_id -> node_id
    assignments: RwLock<HashMap<String, String>>,
    /// 任务 source 映射 task_id -> source_id
    task_sources: RwLock<HashMap<String, String>>,
    /// source 亲和性索引 source_id -> node_id
    source_nodes: RwLock<HashMap<String, String>>,
    /// 集群配置
    config: ClusterConfig,
    /// 心跳提供者
    heartbeat_provider: Arc<dyn HeartbeatProvider>,
    /// 任务分发器
    task_dispatcher: Arc<dyn TaskDispatcher>,
    /// 时间戳函数（便于测试固定时间戳）
    timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    /// 统计：已分配任务总数
    total_assigned: AtomicU64,
    /// 统计：已迁移任务总数
    total_migrated: AtomicU64,
    /// 统计：已下线节点总数
    total_dead_nodes: AtomicU64,
}

impl ClusterCoordinator {
    /// 创建集群协调器（使用 SystemTime 作为时间戳源）
    pub fn new(config: ClusterConfig) -> Result<Self, ClusterError> {
        config.validate()?;
        Ok(Self::with_timestamp_fn(
            config,
            Box::new(current_millis),
            Arc::new(NoopHeartbeatProvider),
            Arc::new(NoopTaskDispatcher),
        ))
    }

    /// 创建集群协调器，注入自定义时间戳函数和提供者
    pub fn with_timestamp_fn(
        config: ClusterConfig,
        timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
        heartbeat_provider: Arc<dyn HeartbeatProvider>,
        task_dispatcher: Arc<dyn TaskDispatcher>,
    ) -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            assignments: RwLock::new(HashMap::new()),
            task_sources: RwLock::new(HashMap::new()),
            source_nodes: RwLock::new(HashMap::new()),
            config,
            heartbeat_provider,
            task_dispatcher,
            timestamp_fn,
            total_assigned: AtomicU64::new(0),
            total_migrated: AtomicU64::new(0),
            total_dead_nodes: AtomicU64::new(0),
        }
    }

    /// 获取配置（不可变引用）
    pub fn config(&self) -> &ClusterConfig {
        &self.config
    }

    // -----------------------------------------------------------------
    // 节点管理 API
    // -----------------------------------------------------------------

    /// 注册节点
    ///
    /// # 参数
    /// - `node_id`：节点 ID（唯一）
    /// - `address`：节点地址（host:port）
    /// - `max_tasks`：该节点最大任务数（0 表示使用配置默认值）
    ///
    /// # 错误
    /// - `NodeAlreadyExists`：节点 ID 已存在
    /// - `InvalidConfig`：node_id 或 address 为空
    pub fn register_node(
        &self,
        node_id: &str,
        address: &str,
        max_tasks: u32,
    ) -> Result<ClusterNode, ClusterError> {
        if node_id.is_empty() {
            return Err(ClusterError::InvalidConfig("node_id is empty".to_string()));
        }
        if address.is_empty() {
            return Err(ClusterError::InvalidConfig("address is empty".to_string()));
        }
        let effective_max = if max_tasks == 0 {
            self.config.max_tasks_per_node
        } else {
            max_tasks
        };

        let now = (self.timestamp_fn)();
        let mut nodes = self.nodes.write();
        if nodes.contains_key(node_id) {
            return Err(ClusterError::NodeAlreadyExists(node_id.to_string()));
        }
        let mut node = ClusterNode::new(node_id, address, effective_max);
        // 使用注入的时间戳覆盖（ClusterNode::new 内部用 SystemTime，测试时需用注入值）
        node.last_heartbeat = now;
        node.registered_at = now;
        nodes.insert(node_id.to_string(), node.clone());
        drop(nodes);

        // 注册后触发 Leader 选举
        let _ = self.elect_leader_internal();
        Ok(node)
    }

    /// 注销节点
    ///
    /// **流程**：
    /// 1. 若节点有任务，先迁移到其他 Alive 节点（若 `enable_task_migration` 为 true）
    /// 2. 从节点列表移除
    /// 3. 从亲和性索引移除
    /// 4. 触发 Leader 选举（若下线的是 Leader）
    ///
    /// # 错误
    /// - `NodeNotFound`：节点不存在
    pub fn unregister_node(&self, node_id: &str) -> Result<(), ClusterError> {
        // 1. 标记节点为 Leaving（防止新任务分配到此节点）
        {
            let mut nodes = self.nodes.write();
            let node = nodes
                .get_mut(node_id)
                .ok_or_else(|| ClusterError::NodeNotFound(node_id.to_string()))?;
            node.status = NodeStatus::Leaving;
        }

        // 2. 迁移该节点的任务
        let task_ids: Vec<String> = {
            let assignments = self.assignments.read();
            assignments
                .iter()
                .filter(|(_, nid)| *nid == node_id)
                .map(|(tid, _)| tid.clone())
                .collect()
        };
        for task_id in task_ids {
            // 尝试迁移到其他节点
            if let Err(e) = self.migrate_task_away(&task_id, node_id) {
                // 迁移失败：取消任务分配
                let _ = self.unassign_task_internal(&task_id);
                tracing_debug(&format!(
                    "migrate task {} away from {} failed: {}",
                    task_id, node_id, e
                ));
            }
        }

        // 3. 从亲和性索引移除
        {
            let mut source_nodes = self.source_nodes.write();
            source_nodes.retain(|_, nid| nid != node_id);
        }

        // 4. 从节点列表移除
        {
            let mut nodes = self.nodes.write();
            nodes.remove(node_id);
        }

        // 5. 触发 Leader 选举
        let _ = self.elect_leader_internal();
        Ok(())
    }

    /// 更新节点心跳
    ///
    /// **效果**：更新节点的 `last_heartbeat`，若节点原为 Dead 则恢复为 Alive
    ///
    /// # 错误
    /// - `NodeNotFound`：节点不存在
    pub fn heartbeat(&self, node_id: &str) -> Result<(), ClusterError> {
        let now = (self.timestamp_fn)();
        let mut nodes = self.nodes.write();
        let node = nodes
            .get_mut(node_id)
            .ok_or_else(|| ClusterError::NodeNotFound(node_id.to_string()))?;
        node.touch_heartbeat(now);
        // Dead 节点恢复心跳 → Alive
        if node.status == NodeStatus::Dead {
            node.status = NodeStatus::Alive;
        }
        drop(nodes);
        // 心跳后触发 Leader 选举（可能 Dead → Alive 的节点成为新 Leader）
        let _ = self.elect_leader_internal();
        Ok(())
    }

    /// 更新节点资源使用情况
    ///
    /// # 参数
    /// - `node_id`：节点 ID
    /// - `cpu_usage`：CPU 使用率（0-100）
    /// - `memory_usage`：内存使用率（0-100）
    ///
    /// # 错误
    /// - `NodeNotFound`：节点不存在
    pub fn update_node_metrics(
        &self,
        node_id: &str,
        cpu_usage: f64,
        memory_usage: f64,
    ) -> Result<(), ClusterError> {
        let mut nodes = self.nodes.write();
        let node = nodes
            .get_mut(node_id)
            .ok_or_else(|| ClusterError::NodeNotFound(node_id.to_string()))?;
        node.capacity.cpu_usage = cpu_usage.clamp(0.0, 100.0);
        node.capacity.memory_usage = memory_usage.clamp(0.0, 100.0);
        Ok(())
    }

    // -----------------------------------------------------------------
    // 任务分配 API
    // -----------------------------------------------------------------

    /// 分配任务
    ///
    /// **算法**（`TaskAssignment::select_node`）：
    /// 1. 亲和性优先：若 `source_id` 已有节点持有，优先分配到该节点
    /// 2. 负载均衡：选择 `load_score` 最低的 Alive 节点
    /// 3. CPU 优先：分数相同时选择 `cpu_usage` 最低
    /// 4. node_id 字典序稳定：完全相同时选最小
    ///
    /// # 参数
    /// - `task_id`：任务 ID（唯一）
    /// - `source_id`：源标识（用于亲和性，空串表示无亲和性）
    ///
    /// # 返回
    /// - `Ok(node_id)`：分配到的节点 ID
    ///
    /// # 错误
    /// - `TaskAlreadyAssigned`：任务已分配
    /// - `NoAvailableNode`：无可用节点
    /// - `TaskDispatcher`：分发器调用失败
    pub fn assign_task(&self, task_id: &str, source_id: &str) -> Result<String, ClusterError> {
        if task_id.is_empty() {
            return Err(ClusterError::InvalidConfig("task_id is empty".to_string()));
        }

        // 1. 检查任务是否已分配
        {
            let assignments = self.assignments.read();
            if let Some(existing_node) = assignments.get(task_id) {
                return Err(ClusterError::TaskAlreadyAssigned {
                    task_id: task_id.to_string(),
                    node_id: existing_node.clone(),
                });
            }
        }

        // 2. 选节点（先读锁）
        let target_node_id = {
            let nodes = self.nodes.read();
            let candidates: Vec<ClusterNode> = nodes
                .values()
                .filter(|n| n.can_accept_tasks())
                .cloned()
                .collect();
            if candidates.is_empty() {
                return Err(ClusterError::NoAvailableNode(
                    "no alive node with available capacity".to_string(),
                ));
            }
            // 亲和性：若 source_id 非空，查亲和性索引
            let affinity_node = if !source_id.is_empty() {
                let source_nodes = self.source_nodes.read();
                source_nodes.get(source_id).cloned()
            } else {
                None
            };
            let selected = TaskAssignment::select_node(&candidates, affinity_node.as_deref())
                .ok_or_else(|| {
                    ClusterError::NoAvailableNode("select_node returned none".to_string())
                })?;
            selected.node_id.clone()
        };

        // 3. 调用分发器（在持锁外，避免网络调用阻塞锁）
        {
            let node_snapshot = {
                let nodes = self.nodes.read();
                nodes
                    .get(&target_node_id)
                    .cloned()
                    .ok_or_else(|| ClusterError::NodeNotFound(target_node_id.clone()))?
            };
            self.task_dispatcher
                .dispatch_task(task_id, source_id, &node_snapshot)
                .map_err(|e| ClusterError::TaskDispatcher(e.to_string()))?;
        }

        // 4. 写入分配映射 + 更新节点容量 + 更新亲和性索引
        {
            let mut assignments = self.assignments.write();
            // 二次检查（防止并发分配）
            if let Some(existing) = assignments.get(task_id) {
                return Err(ClusterError::TaskAlreadyAssigned {
                    task_id: task_id.to_string(),
                    node_id: existing.clone(),
                });
            }
            assignments.insert(task_id.to_string(), target_node_id.clone());
        }
        {
            let mut nodes = self.nodes.write();
            if let Some(node) = nodes.get_mut(&target_node_id) {
                node.capacity.current_tasks = node.capacity.current_tasks.saturating_add(1);
            }
        }
        if !source_id.is_empty() {
            let mut task_sources = self.task_sources.write();
            task_sources.insert(task_id.to_string(), source_id.to_string());
            let mut source_nodes = self.source_nodes.write();
            // 仅当该 source 还没有亲和节点时设置
            source_nodes
                .entry(source_id.to_string())
                .or_insert_with(|| target_node_id.clone());
        }

        self.total_assigned.fetch_add(1, Ordering::SeqCst);
        Ok(target_node_id)
    }

    /// 取消任务分配
    ///
    /// # 错误
    /// - `TaskNotAssigned`：任务未分配
    pub fn unassign_task(&self, task_id: &str) -> Result<(), ClusterError> {
        self.unassign_task_internal(task_id)
    }

    /// 内部取消任务分配（不暴露）
    fn unassign_task_internal(&self, task_id: &str) -> Result<(), ClusterError> {
        // 1. 移除分配映射
        let node_id = {
            let mut assignments = self.assignments.write();
            assignments
                .remove(task_id)
                .ok_or_else(|| ClusterError::TaskNotAssigned(task_id.to_string()))?
        };

        // 2. 通知节点取消任务（持锁外）
        let node_snapshot = {
            let nodes = self.nodes.read();
            nodes.get(&node_id).cloned()
        };
        if let Some(node) = node_snapshot {
            let _ = self.task_dispatcher.undispatch_task(task_id, &node);
        }

        // 3. 减少节点 current_tasks
        {
            let mut nodes = self.nodes.write();
            if let Some(node) = nodes.get_mut(&node_id) {
                node.capacity.current_tasks = node.capacity.current_tasks.saturating_sub(1);
            }
        }

        // 4. 移除任务的 source 映射（亲和性索引保留，便于同源任务仍可复用）
        {
            let mut task_sources = self.task_sources.write();
            task_sources.remove(task_id);
        }
        Ok(())
    }

    /// 迁移任务到指定节点
    ///
    /// # 参数
    /// - `task_id`：任务 ID
    /// - `target_node_id`：目标节点 ID
    ///
    /// # 错误
    /// - `TaskNotAssigned`：任务未分配
    /// - `NodeNotFound`：目标节点不存在
    /// - `NodeDead`：目标节点已死亡
    /// - `NodeCapacityFull`：目标节点容量已满
    pub fn migrate_task(&self, task_id: &str, target_node_id: &str) -> Result<(), ClusterError> {
        // 1. 校验目标节点
        {
            let nodes = self.nodes.read();
            let node = nodes
                .get(target_node_id)
                .ok_or_else(|| ClusterError::NodeNotFound(target_node_id.to_string()))?;
            if node.status == NodeStatus::Dead {
                return Err(ClusterError::NodeDead(target_node_id.to_string()));
            }
            if node.status == NodeStatus::Leaving {
                return Err(ClusterError::NodeLeaving(target_node_id.to_string()));
            }
            if !node.capacity.has_capacity() {
                return Err(ClusterError::NodeCapacityFull(target_node_id.to_string()));
            }
        }

        // 2. 获取当前分配的源节点
        let source_node_id = {
            let assignments = self.assignments.read();
            assignments
                .get(task_id)
                .cloned()
                .ok_or_else(|| ClusterError::TaskNotAssigned(task_id.to_string()))?
        };
        if source_node_id == target_node_id {
            // 已在目标节点，无需迁移
            return Ok(());
        }

        // 3. 调用分发器（持锁外）
        let (source_node_snap, target_node_snap) = {
            let nodes = self.nodes.read();
            let src = nodes
                .get(&source_node_id)
                .ok_or_else(|| ClusterError::NodeNotFound(source_node_id.clone()))?
                .clone();
            let tgt = nodes
                .get(target_node_id)
                .ok_or_else(|| ClusterError::NodeNotFound(target_node_id.to_string()))?
                .clone();
            (src, tgt)
        };
        self.task_dispatcher
            .migrate_task(task_id, &source_node_snap, &target_node_snap)
            .map_err(|e| ClusterError::TaskDispatcher(e.to_string()))?;

        // 4. 更新分配映射 + 容量
        {
            let mut assignments = self.assignments.write();
            assignments.insert(task_id.to_string(), target_node_id.to_string());
        }
        {
            let mut nodes = self.nodes.write();
            if let Some(node) = nodes.get_mut(&source_node_id) {
                node.capacity.current_tasks = node.capacity.current_tasks.saturating_sub(1);
            }
            if let Some(node) = nodes.get_mut(target_node_id) {
                node.capacity.current_tasks = node.capacity.current_tasks.saturating_add(1);
            }
        }
        // 5. 更新亲和性索引（若该任务有 source_id）
        {
            let task_sources = self.task_sources.read();
            if let Some(source_id) = task_sources.get(task_id) {
                let mut source_nodes = self.source_nodes.write();
                source_nodes.insert(source_id.clone(), target_node_id.to_string());
            }
        }

        self.total_migrated.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 将任务从指定节点迁移走（节点下线时使用）
    ///
    /// 选择其他 Alive 且有容量的节点作为目标
    fn migrate_task_away(&self, task_id: &str, from_node_id: &str) -> Result<(), ClusterError> {
        // 选目标节点（排除 from_node_id）
        let target_node_id = {
            let nodes = self.nodes.read();
            let candidates: Vec<ClusterNode> = nodes
                .values()
                .filter(|n| n.node_id != from_node_id && n.can_accept_tasks())
                .cloned()
                .collect();
            if candidates.is_empty() {
                return Err(ClusterError::NoAvailableNode(format!(
                    "no available node to migrate task {} from {}",
                    task_id, from_node_id
                )));
            }
            // 亲和性：若任务有 source_id，优先选同源节点
            let affinity_node = {
                let task_sources = self.task_sources.read();
                task_sources.get(task_id).and_then(|sid| {
                    let source_nodes = self.source_nodes.read();
                    source_nodes.get(sid).cloned()
                })
            };
            let selected = TaskAssignment::select_node(&candidates, affinity_node.as_deref())
                .ok_or_else(|| {
                    ClusterError::NoAvailableNode(
                        "select_node returned none for migration".to_string(),
                    )
                })?;
            selected.node_id.clone()
        };
        self.migrate_task(task_id, &target_node_id)
    }

    // -----------------------------------------------------------------
    // 心跳检测 API
    // -----------------------------------------------------------------

    /// 检查所有节点心跳，将超时节点标记为 Dead 并迁移其任务
    ///
    /// **流程**：
    /// 1. 遍历所有节点，检查 `last_heartbeat` 是否超时
    /// 2. 超时节点：Alive/Leaving → Dead
    /// 3. 若 `enable_task_migration` 为 true，迁移 Dead 节点的任务
    /// 4. 触发 Leader 选举
    ///
    /// # 返回
    /// - `Ok(Vec<String>)`：本次被标记为 Dead 的节点 ID 列表
    pub fn check_heartbeats(&self) -> Vec<String> {
        let now = (self.timestamp_fn)();
        let mut dead_nodes = Vec::new();

        // 1. 标记超时节点为 Dead
        {
            let mut nodes = self.nodes.write();
            for node in nodes.values_mut() {
                if node.status == NodeStatus::Alive
                    && node.is_heartbeat_timeout(now, self.config.heartbeat_timeout_ms)
                {
                    node.status = NodeStatus::Dead;
                    dead_nodes.push(node.node_id.clone());
                    self.total_dead_nodes.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        // 2. 迁移 Dead 节点的任务
        if self.config.enable_task_migration && !dead_nodes.is_empty() {
            for dead_node_id in &dead_nodes {
                let task_ids: Vec<String> = {
                    let assignments = self.assignments.read();
                    assignments
                        .iter()
                        .filter(|(_, nid)| *nid == dead_node_id)
                        .map(|(tid, _)| tid.clone())
                        .collect()
                };
                for task_id in task_ids {
                    if let Err(e) = self.migrate_task_away(&task_id, dead_node_id) {
                        // 迁移失败：取消任务分配
                        let _ = self.unassign_task_internal(&task_id);
                        tracing_debug(&format!(
                            "auto-migrate task {} from dead node {} failed: {}",
                            task_id, dead_node_id, e
                        ));
                    }
                }
            }
        }

        // 3. 触发 Leader 选举
        let _ = self.elect_leader_internal();
        dead_nodes
    }

    // -----------------------------------------------------------------
    // 选举 API
    // -----------------------------------------------------------------

    /// 简化的 Leader 选举：node_id 字典序最小的 Alive 节点为 Leader
    ///
    /// **流程**：
    /// 1. 收集所有 Alive 节点
    /// 2. 选 node_id 字典序最小的为 Leader
    /// 3. 其余 Alive 节点设为 Follower
    /// 4. 非 Alive 节点保持原角色
    ///
    /// # 返回
    /// - `Ok(Option<String>)`：新 Leader 的 node_id（None 表示无 Alive 节点）
    pub fn elect_leader(&self) -> Option<String> {
        self.elect_leader_internal()
    }

    /// 内部 Leader 选举实现
    fn elect_leader_internal(&self) -> Option<String> {
        let mut nodes = self.nodes.write();
        // 找出 node_id 字典序最小的 Alive 节点
        let leader_id = nodes
            .values()
            .filter(|n| n.status == NodeStatus::Alive)
            .map(|n| n.node_id.as_str())
            .min()
            .map(|s| s.to_string());

        // 更新所有节点的角色
        for node in nodes.values_mut() {
            if node.status != NodeStatus::Alive {
                // 非 Alive 节点不参与选举，角色设为 Follower（保守）
                node.role = NodeRole::Follower;
                continue;
            }
            node.role = if Some(&node.node_id) == leader_id.as_ref() {
                NodeRole::Leader
            } else {
                NodeRole::Follower
            };
        }
        leader_id
    }

    /// 获取当前 Leader
    pub fn current_leader(&self) -> Option<String> {
        let nodes = self.nodes.read();
        nodes
            .values()
            .find(|n| n.role == NodeRole::Leader && n.status == NodeStatus::Alive)
            .map(|n| n.node_id.clone())
    }

    // -----------------------------------------------------------------
    // 查询 API
    // -----------------------------------------------------------------

    /// 获取节点信息
    pub fn get_node(&self, node_id: &str) -> Option<ClusterNode> {
        let nodes = self.nodes.read();
        nodes.get(node_id).cloned()
    }

    /// 列出所有节点
    pub fn list_nodes(&self) -> Vec<ClusterNode> {
        let nodes = self.nodes.read();
        let mut list: Vec<ClusterNode> = nodes.values().cloned().collect();
        // 按 node_id 字典序排序，保证输出稳定
        list.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        list
    }

    /// 列出所有任务分配（task_id -> node_id）
    pub fn list_assignments(&self) -> Vec<(String, String)> {
        let assignments = self.assignments.read();
        let mut list: Vec<(String, String)> = assignments
            .iter()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// 获取任务分配的节点 ID
    pub fn get_assignment(&self, task_id: &str) -> Option<String> {
        let assignments = self.assignments.read();
        assignments.get(task_id).cloned()
    }

    /// 获取任务的 source_id
    pub fn get_task_source(&self, task_id: &str) -> Option<String> {
        let task_sources = self.task_sources.read();
        task_sources.get(task_id).cloned()
    }

    /// 获取 source 的亲和节点
    pub fn get_source_affinity(&self, source_id: &str) -> Option<String> {
        let source_nodes = self.source_nodes.read();
        source_nodes.get(source_id).cloned()
    }

    /// 节点数量
    pub fn node_count(&self) -> usize {
        self.nodes.read().len()
    }

    /// 任务分配数量
    pub fn assignment_count(&self) -> usize {
        self.assignments.read().len()
    }

    /// 已分配任务总数（累计）
    pub fn total_assigned(&self) -> u64 {
        self.total_assigned.load(Ordering::SeqCst)
    }

    /// 已迁移任务总数（累计）
    pub fn total_migrated(&self) -> u64 {
        self.total_migrated.load(Ordering::SeqCst)
    }

    /// 已下线节点总数（累计）
    pub fn total_dead_nodes(&self) -> u64 {
        self.total_dead_nodes.load(Ordering::SeqCst)
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 当前 Unix 毫秒
fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 简易 debug 日志（避免依赖 log/tracing crate）
fn tracing_debug(msg: &str) {
    // 生产环境可替换为 tracing::debug!(...)
    let _ = msg;
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::AtomicU64;
    use std::thread;

    // -----------------------------------------------------------------
    // 辅助：固定时间戳的协调器
    // -----------------------------------------------------------------

    /// 创建测试用协调器：时间戳固定为 1000
    fn make_test_coordinator(config: ClusterConfig) -> ClusterCoordinator {
        ClusterCoordinator::with_timestamp_fn(
            config,
            Box::new(|| 1000),
            Arc::new(NoopHeartbeatProvider),
            Arc::new(NoopTaskDispatcher),
        )
    }

    /// 创建测试用协调器：时间戳可通过返回的 AtomicU64 外部控制
    ///
    /// 用法：`let (coord, time) = make_controllable_coord(cfg);`
    /// - 注册节点时 time 为初始值 1000
    /// - 测试中 `time.store(50000, Ordering::SeqCst)` 推进时间
    fn make_controllable_coord(config: ClusterConfig) -> (ClusterCoordinator, Arc<AtomicU64>) {
        let time = Arc::new(AtomicU64::new(1000));
        let time_clone = time.clone();
        let coord = ClusterCoordinator::with_timestamp_fn(
            config,
            Box::new(move || time_clone.load(Ordering::SeqCst)),
            Arc::new(NoopHeartbeatProvider),
            Arc::new(NoopTaskDispatcher),
        );
        (coord, time)
    }

    /// 注册 3 个节点的辅助函数
    fn register_three_nodes(coord: &ClusterCoordinator) {
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        coord.register_node("node-3", "10.0.0.3:8080", 10).unwrap();
    }

    // =================================================================
    // 1. ClusterConfig 测试
    // =================================================================

    #[test]
    fn config_default_values() {
        let cfg = ClusterConfig::default();
        assert_eq!(cfg.heartbeat_interval_ms, 10_000);
        assert_eq!(cfg.heartbeat_timeout_ms, 30_000);
        assert_eq!(cfg.max_tasks_per_node, 10);
        assert!(cfg.enable_task_migration);
    }

    #[test]
    fn config_builder_chain() {
        let cfg = ClusterConfig::new()
            .with_heartbeat_interval_ms(5000)
            .with_heartbeat_timeout_ms(15000)
            .with_max_tasks_per_node(5)
            .with_task_migration(false);
        assert_eq!(cfg.heartbeat_interval_ms, 5000);
        assert_eq!(cfg.heartbeat_timeout_ms, 15000);
        assert_eq!(cfg.max_tasks_per_node, 5);
        assert!(!cfg.enable_task_migration);
    }

    #[test]
    fn config_validate_rejects_zero_interval() {
        let cfg = ClusterConfig {
            heartbeat_interval_ms: 0,
            heartbeat_timeout_ms: 1000,
            max_tasks_per_node: 10,
            enable_task_migration: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_rejects_timeout_less_than_interval() {
        let cfg = ClusterConfig {
            heartbeat_interval_ms: 10000,
            heartbeat_timeout_ms: 5000,
            max_tasks_per_node: 10,
            enable_task_migration: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_rejects_zero_max_tasks() {
        let cfg = ClusterConfig {
            heartbeat_interval_ms: 10000,
            heartbeat_timeout_ms: 30000,
            max_tasks_per_node: 0,
            enable_task_migration: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_accepts_valid() {
        let cfg = ClusterConfig::default();
        assert!(cfg.validate().is_ok());
    }

    // =================================================================
    // 2. NodeRole / NodeStatus 测试
    // =================================================================

    #[test]
    fn node_role_as_str() {
        assert_eq!(NodeRole::Leader.as_str(), "leader");
        assert_eq!(NodeRole::Follower.as_str(), "follower");
        assert_eq!(NodeRole::Candidate.as_str(), "candidate");
    }

    #[test]
    fn node_status_can_accept_tasks() {
        assert!(NodeStatus::Alive.can_accept_tasks());
        assert!(!NodeStatus::Dead.can_accept_tasks());
        assert!(!NodeStatus::Leaving.can_accept_tasks());
    }

    #[test]
    fn node_status_is_terminal() {
        assert!(NodeStatus::Dead.is_terminal());
        assert!(!NodeStatus::Alive.is_terminal());
        assert!(!NodeStatus::Leaving.is_terminal());
    }

    // =================================================================
    // 3. NodeCapacity 测试
    // =================================================================

    #[test]
    fn node_capacity_has_capacity() {
        let cap = NodeCapacity::new(10);
        assert!(cap.has_capacity());
        assert_eq!(cap.available_capacity(), 10);
    }

    #[test]
    fn node_capacity_no_capacity_when_full() {
        let mut cap = NodeCapacity::new(2);
        cap.current_tasks = 2;
        assert!(!cap.has_capacity());
        assert_eq!(cap.available_capacity(), 0);
    }

    #[test]
    fn node_capacity_load_score_increases_with_tasks() {
        let mut cap1 = NodeCapacity::new(10);
        cap1.current_tasks = 1;
        let mut cap2 = NodeCapacity::new(10);
        cap2.current_tasks = 5;
        assert!(cap1.load_score() < cap2.load_score());
    }

    // =================================================================
    // 4. ClusterNode 测试
    // =================================================================

    #[test]
    fn cluster_node_new_is_alive_follower() {
        let node = ClusterNode::new("node-1", "10.0.0.1:8080", 10);
        assert_eq!(node.node_id, "node-1");
        assert_eq!(node.address, "10.0.0.1:8080");
        assert_eq!(node.role, NodeRole::Follower);
        assert_eq!(node.status, NodeStatus::Alive);
        assert_eq!(node.capacity.max_tasks, 10);
        assert_eq!(node.capacity.current_tasks, 0);
        assert!(node.is_alive());
        assert!(!node.is_dead());
        assert!(node.can_accept_tasks());
    }

    #[test]
    fn cluster_node_heartbeat_timeout_detection() {
        let node = ClusterNode {
            node_id: "node-1".to_string(),
            address: "10.0.0.1:8080".to_string(),
            role: NodeRole::Follower,
            capacity: NodeCapacity::new(10),
            status: NodeStatus::Alive,
            last_heartbeat: 1000,
            registered_at: 1000,
        };
        // 1000 + 30000 = 31000 时不超时
        assert!(!node.is_heartbeat_timeout(31_000, 30_000));
        // 1000 + 30001 = 31001 时超时
        assert!(node.is_heartbeat_timeout(31_001, 30_000));
    }

    // =================================================================
    // 5. 节点注册/注销测试
    // =================================================================

    #[test]
    fn register_node_success() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let node = coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        assert_eq!(node.node_id, "node-1");
        assert_eq!(coord.node_count(), 1);
    }

    #[test]
    fn register_node_duplicate_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        let result = coord.register_node("node-1", "10.0.0.1:9090", 5);
        assert!(matches!(result, Err(ClusterError::NodeAlreadyExists(_))));
        assert_eq!(coord.node_count(), 1);
    }

    #[test]
    fn register_node_empty_id_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let result = coord.register_node("", "10.0.0.1:8080", 10);
        assert!(matches!(result, Err(ClusterError::InvalidConfig(_))));
    }

    #[test]
    fn register_node_empty_address_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let result = coord.register_node("node-1", "", 10);
        assert!(matches!(result, Err(ClusterError::InvalidConfig(_))));
    }

    #[test]
    fn register_node_zero_max_uses_config_default() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let node = coord.register_node("node-1", "10.0.0.1:8080", 0).unwrap();
        assert_eq!(node.capacity.max_tasks, 10);
    }

    #[test]
    fn unregister_node_success() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        assert_eq!(coord.node_count(), 1);
        coord.unregister_node("node-1").unwrap();
        assert_eq!(coord.node_count(), 0);
    }

    #[test]
    fn unregister_nonexistent_node_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let result = coord.unregister_node("node-x");
        assert!(matches!(result, Err(ClusterError::NodeNotFound(_))));
    }

    #[test]
    fn list_nodes_sorted_by_id() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-c", "10.0.0.3:8080", 10).unwrap();
        coord.register_node("node-a", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-b", "10.0.0.2:8080", 10).unwrap();
        let list = coord.list_nodes();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].node_id, "node-a");
        assert_eq!(list[1].node_id, "node-b");
        assert_eq!(list[2].node_id, "node-c");
    }

    #[test]
    fn get_node_returns_clone() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.address, "10.0.0.1:8080");
        assert!(coord.get_node("node-x").is_none());
    }

    // =================================================================
    // 6. 心跳检测与超时标记测试
    // =================================================================

    #[test]
    fn heartbeat_updates_last_heartbeat() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        // 时间戳固定为 1000，所以 last_heartbeat 应该是 1000
        coord.heartbeat("node-1").unwrap();
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.last_heartbeat, 1000);
    }

    #[test]
    fn heartbeat_nonexistent_node_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let result = coord.heartbeat("node-x");
        assert!(matches!(result, Err(ClusterError::NodeNotFound(_))));
    }

    #[test]
    fn heartbeat_revives_dead_node() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        // 标记为 Dead（手动设置）
        {
            let mut nodes = coord.nodes.write();
            nodes.get_mut("node-1").unwrap().status = NodeStatus::Dead;
        }
        // 心跳恢复
        coord.heartbeat("node-1").unwrap();
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.status, NodeStatus::Alive);
    }

    #[test]
    fn check_heartbeats_marks_timeout_nodes_dead() {
        let (coord, time) = make_controllable_coord(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        // 注册时 last_heartbeat=1000
        // 推进时间到 50000，50000 - 1000 = 49000 > 30000，超时
        time.store(50_000, Ordering::SeqCst);
        let dead = coord.check_heartbeats();
        assert_eq!(dead, vec!["node-1".to_string()]);
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.status, NodeStatus::Dead);
    }

    #[test]
    fn check_heartbeats_does_not_mark_recent_nodes_dead() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        // 时间戳始终 1000，注册时 last_heartbeat=1000，check 时 now=1000
        let dead = coord.check_heartbeats();
        assert!(dead.is_empty());
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.status, NodeStatus::Alive);
    }

    // =================================================================
    // 7. 任务分配测试（负载均衡、亲和性）
    // =================================================================

    #[test]
    fn assign_task_to_single_node() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        let node_id = coord.assign_task("task-1", "source-1").unwrap();
        assert_eq!(node_id, "node-1");
        assert_eq!(coord.assignment_count(), 1);
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.capacity.current_tasks, 1);
    }

    #[test]
    fn assign_task_balances_load() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        // 3 个节点初始 cpu_usage=0，load_score 相同，选 node_id 字典序最小的 node-1
        let n1 = coord.assign_task("task-1", "").unwrap();
        assert_eq!(n1, "node-1");
        // node-1 current_tasks=1，load_score 更高，选 node-2
        let n2 = coord.assign_task("task-2", "").unwrap();
        assert_eq!(n2, "node-2");
        // node-1, node-2 current_tasks=1，选 node-3
        let n3 = coord.assign_task("task-3", "").unwrap();
        assert_eq!(n3, "node-3");
        // 全部 current_tasks=1，选 node_id 字典序最小的 node-1
        let n4 = coord.assign_task("task-4", "").unwrap();
        assert_eq!(n4, "node-1");
    }

    #[test]
    fn assign_task_affinity_same_source_goes_same_node() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        // 第一个任务分配到 node-1
        let n1 = coord.assign_task("task-1", "pg-source-1").unwrap();
        // 同源任务应分配到同节点
        let n2 = coord.assign_task("task-2", "pg-source-1").unwrap();
        assert_eq!(n1, n2);
        // 不同源任务可能分配到不同节点
        let n3 = coord.assign_task("task-3", "pg-source-2").unwrap();
        // task-3 应分配到 load_score 最低的节点（node-2 或 node-3）
        assert_ne!(n3, n1);
    }

    #[test]
    fn assign_task_different_sources_can_distribute() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        let n1 = coord.assign_task("task-1", "source-a").unwrap();
        let n2 = coord.assign_task("task-2", "source-b").unwrap();
        let n3 = coord.assign_task("task-3", "source-c").unwrap();
        // 三个不同源任务应分配到三个不同节点（负载均衡）
        assert_ne!(n1, n2);
        assert_ne!(n1, n3);
        assert_ne!(n2, n3);
    }

    #[test]
    fn assign_task_duplicate_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "source-1").unwrap();
        let result = coord.assign_task("task-1", "source-1");
        assert!(matches!(
            result,
            Err(ClusterError::TaskAlreadyAssigned { .. })
        ));
    }

    #[test]
    fn assign_task_no_nodes_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let result = coord.assign_task("task-1", "source-1");
        assert!(matches!(result, Err(ClusterError::NoAvailableNode(_))));
    }

    #[test]
    fn assign_task_all_nodes_full_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 1).unwrap();
        // 占满容量
        coord.assign_task("task-1", "").unwrap();
        // 再分配应失败
        let result = coord.assign_task("task-2", "");
        assert!(matches!(result, Err(ClusterError::NoAvailableNode(_))));
    }

    #[test]
    fn assign_task_skips_dead_node() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        // 标记 node-1 为 Dead
        {
            let mut nodes = coord.nodes.write();
            nodes.get_mut("node-1").unwrap().status = NodeStatus::Dead;
        }
        // 应分配到 node-2
        let node_id = coord.assign_task("task-1", "").unwrap();
        assert_eq!(node_id, "node-2");
    }

    #[test]
    fn assign_task_empty_task_id_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        let result = coord.assign_task("", "source-1");
        assert!(matches!(result, Err(ClusterError::InvalidConfig(_))));
    }

    #[test]
    fn unassign_task_decreases_current_tasks() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "source-1").unwrap();
        assert_eq!(coord.get_node("node-1").unwrap().capacity.current_tasks, 1);
        coord.unassign_task("task-1").unwrap();
        assert_eq!(coord.get_node("node-1").unwrap().capacity.current_tasks, 0);
        assert_eq!(coord.assignment_count(), 0);
    }

    #[test]
    fn unassign_nonexistent_task_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        let result = coord.unassign_task("task-x");
        assert!(matches!(result, Err(ClusterError::TaskNotAssigned(_))));
    }

    #[test]
    fn get_assignment_returns_node_id() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "source-1").unwrap();
        assert_eq!(coord.get_assignment("task-1"), Some("node-1".to_string()));
        assert_eq!(coord.get_assignment("task-x"), None);
    }

    #[test]
    fn list_assignments_sorted_by_task_id() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-c", "").unwrap();
        coord.assign_task("task-a", "").unwrap();
        coord.assign_task("task-b", "").unwrap();
        let list = coord.list_assignments();
        assert_eq!(list[0].0, "task-a");
        assert_eq!(list[1].0, "task-b");
        assert_eq!(list[2].0, "task-c");
    }

    // =================================================================
    // 8. 任务迁移测试
    // =================================================================

    #[test]
    fn migrate_task_to_specific_node() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        // 初始分配到 node-1
        let n1 = coord.assign_task("task-1", "").unwrap();
        assert_eq!(n1, "node-1");
        // 迁移到 node-2
        coord.migrate_task("task-1", "node-2").unwrap();
        assert_eq!(coord.get_assignment("task-1"), Some("node-2".to_string()));
        // 容量更新
        assert_eq!(coord.get_node("node-1").unwrap().capacity.current_tasks, 0);
        assert_eq!(coord.get_node("node-2").unwrap().capacity.current_tasks, 1);
        assert_eq!(coord.total_migrated(), 1);
    }

    #[test]
    fn migrate_task_to_same_node_noop() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "").unwrap();
        // 迁移到同节点应成功（noop）
        coord.migrate_task("task-1", "node-1").unwrap();
        assert_eq!(coord.get_assignment("task-1"), Some("node-1".to_string()));
    }

    #[test]
    fn migrate_nonexistent_task_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        let result = coord.migrate_task("task-x", "node-1");
        assert!(matches!(result, Err(ClusterError::TaskNotAssigned(_))));
    }

    #[test]
    fn migrate_to_nonexistent_node_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "").unwrap();
        let result = coord.migrate_task("task-1", "node-x");
        assert!(matches!(result, Err(ClusterError::NodeNotFound(_))));
    }

    #[test]
    fn migrate_to_dead_node_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        coord.assign_task("task-1", "").unwrap();
        // 标记 node-2 为 Dead
        {
            let mut nodes = coord.nodes.write();
            nodes.get_mut("node-2").unwrap().status = NodeStatus::Dead;
        }
        let result = coord.migrate_task("task-1", "node-2");
        assert!(matches!(result, Err(ClusterError::NodeDead(_))));
    }

    #[test]
    fn migrate_to_full_node_fails() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 1).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 1).unwrap();
        // 占满 node-2
        coord.assign_task("task-2", "").unwrap();
        // node-1 上有 task-1，迁移到 node-2（已满）应失败
        coord.assign_task("task-1", "").unwrap();
        let result = coord.migrate_task("task-1", "node-2");
        assert!(matches!(result, Err(ClusterError::NodeCapacityFull(_))));
    }

    #[test]
    fn unregister_node_migrates_tasks() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        // 在 node-1 上分配任务
        coord.assign_task("task-1", "").unwrap();
        assert_eq!(coord.get_assignment("task-1"), Some("node-1".to_string()));
        // 注销 node-1，任务应迁移到其他节点
        coord.unregister_node("node-1").unwrap();
        let new_node = coord
            .get_assignment("task-1")
            .expect("task should be migrated");
        assert_ne!(new_node, "node-1");
        assert_eq!(coord.node_count(), 2);
    }

    #[test]
    fn dead_node_tasks_auto_migrated_on_check_heartbeats() {
        let (coord, time) = make_controllable_coord(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        // 任务分配到 node-1（字典序最小）
        coord.assign_task("task-1", "").unwrap();
        assert_eq!(coord.get_assignment("task-1"), Some("node-1".to_string()));
        // 推进时间到 40000，给 node-2 发心跳（保持存活）
        time.store(40_000, Ordering::SeqCst);
        coord.heartbeat("node-2").unwrap();
        // 再推进到 50000，node-1 超时（50000-1000=49000>30000），node-2 存活（50000-40000=10000<30000）
        time.store(50_000, Ordering::SeqCst);
        let dead = coord.check_heartbeats();
        assert_eq!(dead, vec!["node-1".to_string()]);
        // 任务自动迁移到 node-2
        assert_eq!(coord.get_assignment("task-1"), Some("node-2".to_string()));
    }

    #[test]
    fn migration_disabled_when_config_disabled() {
        let cfg = ClusterConfig {
            heartbeat_interval_ms: 10_000,
            heartbeat_timeout_ms: 30_000,
            max_tasks_per_node: 10,
            enable_task_migration: false,
        };
        let (coord, time) = make_controllable_coord(cfg);
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        coord.assign_task("task-1", "").unwrap();
        // 推进时间，给 node-2 发心跳保持存活
        time.store(40_000, Ordering::SeqCst);
        coord.heartbeat("node-2").unwrap();
        // 再推进到 50000，node-1 超时 → Dead，但迁移被禁用
        time.store(50_000, Ordering::SeqCst);
        let dead = coord.check_heartbeats();
        assert_eq!(dead, vec!["node-1".to_string()]);
        // 任务仍分配在 node-1（Dead 节点），未迁移
        assert_eq!(coord.get_assignment("task-1"), Some("node-1".to_string()));
    }

    // =================================================================
    // 9. Leader 选举测试
    // =================================================================

    #[test]
    fn elect_leader_picks_smallest_node_id() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-3", "10.0.0.3:8080", 10).unwrap();
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        // 注册时已触发选举，应选 node-1
        assert_eq!(coord.current_leader(), Some("node-1".to_string()));
    }

    #[test]
    fn elect_leader_no_alive_nodes_returns_none() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        // 标记为 Dead
        {
            let mut nodes = coord.nodes.write();
            nodes.get_mut("node-1").unwrap().status = NodeStatus::Dead;
        }
        let leader = coord.elect_leader();
        assert!(leader.is_none());
        assert!(coord.current_leader().is_none());
    }

    #[test]
    fn elect_leader_changes_when_leader_dies() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        assert_eq!(coord.current_leader(), Some("node-1".to_string()));
        // node-1 死亡
        {
            let mut nodes = coord.nodes.write();
            nodes.get_mut("node-1").unwrap().status = NodeStatus::Dead;
        }
        // 触发选举
        let new_leader = coord.elect_leader();
        assert_eq!(new_leader, Some("node-2".to_string()));
        assert_eq!(coord.current_leader(), Some("node-2".to_string()));
    }

    #[test]
    fn elect_leader_no_nodes_returns_none() {
        let coord = make_test_coordinator(ClusterConfig::default());
        assert!(coord.elect_leader().is_none());
        assert!(coord.current_leader().is_none());
    }

    // =================================================================
    // 10. 并发安全测试
    // =================================================================

    #[test]
    fn concurrent_register_nodes() {
        let coord = Arc::new(make_test_coordinator(ClusterConfig::default()));
        let mut handles = Vec::new();
        for i in 0..10 {
            let coord_clone = coord.clone();
            handles.push(thread::spawn(move || {
                let node_id = format!("node-{}", i);
                coord_clone
                    .register_node(&node_id, "10.0.0.1:8080", 10)
                    .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(coord.node_count(), 10);
    }

    #[test]
    fn concurrent_assign_tasks() {
        let coord = Arc::new(make_test_coordinator(ClusterConfig::default()));
        // 注册 5 个节点，每个容量 20
        for i in 0..5 {
            let node_id = format!("node-{}", i);
            coord.register_node(&node_id, "10.0.0.1:8080", 20).unwrap();
        }
        let mut handles = Vec::new();
        for i in 0..50 {
            let coord_clone = coord.clone();
            handles.push(thread::spawn(move || {
                let task_id = format!("task-{}", i);
                coord_clone.assign_task(&task_id, "").unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(coord.assignment_count(), 50);
        // 所有任务都应分配到节点
        let total_tasks: u32 = coord
            .list_nodes()
            .iter()
            .map(|n| n.capacity.current_tasks)
            .sum();
        assert_eq!(total_tasks, 50);
    }

    #[test]
    fn concurrent_register_and_assign() {
        let coord = Arc::new(make_test_coordinator(ClusterConfig::default()));
        let mut handles = Vec::new();
        // 一半线程注册节点，一半线程分配任务
        for i in 0..10 {
            let coord_clone = coord.clone();
            handles.push(thread::spawn(move || {
                if i < 5 {
                    let node_id = format!("node-{}", i);
                    let _ = coord_clone.register_node(&node_id, "10.0.0.1:8080", 100);
                } else {
                    // 等待节点注册
                    thread::sleep(std::time::Duration::from_millis(10));
                    let task_id = format!("task-{}", i);
                    let _ = coord_clone.assign_task(&task_id, "");
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 至少注册了 5 个节点
        assert!(coord.node_count() >= 5);
    }

    // =================================================================
    // 11. 配置参数生效测试
    // =================================================================

    #[test]
    fn config_max_tasks_per_node_limits_assignments() {
        let coord = make_test_coordinator(ClusterConfig::default());
        // 注册 1 个节点，max_tasks=2
        coord.register_node("node-1", "10.0.0.1:8080", 2).unwrap();
        coord.assign_task("task-1", "").unwrap();
        coord.assign_task("task-2", "").unwrap();
        // 第三个任务应失败（容量已满）
        let result = coord.assign_task("task-3", "");
        assert!(matches!(result, Err(ClusterError::NoAvailableNode(_))));
    }

    #[test]
    fn config_heartbeat_timeout_affects_check() {
        let cfg = ClusterConfig::default().with_heartbeat_timeout_ms(5000);
        let (coord, time) = make_controllable_coord(cfg);
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        // 推进时间到 10000，10000 - 1000 = 9000 > 5000，超时
        time.store(10_000, Ordering::SeqCst);
        let dead = coord.check_heartbeats();
        assert_eq!(dead.len(), 1);
    }

    #[test]
    fn config_zero_max_tasks_per_node_rejected() {
        let cfg = ClusterConfig {
            heartbeat_interval_ms: 10_000,
            heartbeat_timeout_ms: 30_000,
            max_tasks_per_node: 0,
            enable_task_migration: true,
        };
        let result = ClusterCoordinator::new(cfg);
        assert!(result.is_err());
    }

    // =================================================================
    // 12. 统计与查询 API 测试
    // =================================================================

    #[test]
    fn total_assigned_increments() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        assert_eq!(coord.total_assigned(), 0);
        coord.assign_task("task-1", "").unwrap();
        assert_eq!(coord.total_assigned(), 1);
        coord.assign_task("task-2", "").unwrap();
        assert_eq!(coord.total_assigned(), 2);
    }

    #[test]
    fn total_migrated_increments() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        coord.assign_task("task-1", "").unwrap();
        assert_eq!(coord.total_migrated(), 0);
        coord.migrate_task("task-1", "node-2").unwrap();
        assert_eq!(coord.total_migrated(), 1);
    }

    #[test]
    fn total_dead_nodes_increments() {
        let (coord, time) = make_controllable_coord(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        assert_eq!(coord.total_dead_nodes(), 0);
        // 推进时间到 50000，两个节点都超时（50000-1000=49000>30000）
        time.store(50_000, Ordering::SeqCst);
        coord.check_heartbeats();
        assert_eq!(coord.total_dead_nodes(), 2);
    }

    #[test]
    fn get_task_source_returns_source_id() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "pg-source-1").unwrap();
        assert_eq!(
            coord.get_task_source("task-1"),
            Some("pg-source-1".to_string())
        );
        assert_eq!(coord.get_task_source("task-x"), None);
    }

    #[test]
    fn get_source_affinity_returns_node_id() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        coord.assign_task("task-1", "pg-source-1").unwrap();
        let affinity = coord.get_source_affinity("pg-source-1");
        assert!(affinity.is_some());
        assert_eq!(affinity, coord.get_assignment("task-1"));
    }

    #[test]
    fn update_node_metrics_clamps_values() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.update_node_metrics("node-1", 150.0, -10.0).unwrap();
        let node = coord.get_node("node-1").unwrap();
        assert_eq!(node.capacity.cpu_usage, 100.0);
        assert_eq!(node.capacity.memory_usage, 0.0);
    }

    #[test]
    fn update_node_metrics_affects_assignment() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        // node-2 的 CPU 最高
        coord.update_node_metrics("node-2", 90.0, 50.0).unwrap();
        // 第一个任务：node-1, node-3 cpu=0，选 node_id 字典序最小的 node-1
        let n1 = coord.assign_task("task-1", "").unwrap();
        assert_eq!(n1, "node-1");
        // 第二个任务：node-1 已有 1 任务，node-3 cpu=0，node-2 cpu=90
        // node-1 load_score = 0.6*0.1 + 0.4*0 = 0.06
        // node-3 load_score = 0.6*0 + 0.4*0 = 0
        // node-2 load_score = 0.6*0 + 0.4*0.9 = 0.36
        // 选 node-3
        let n2 = coord.assign_task("task-2", "").unwrap();
        assert_eq!(n2, "node-3");
    }

    // =================================================================
    // 13. HeartbeatProvider / TaskDispatcher 注入测试
    // =================================================================

    /// 记录型心跳提供者（记录调用次数）
    struct CountingHeartbeatProvider {
        send_count: AtomicU64,
        check_count: AtomicU64,
    }

    impl CountingHeartbeatProvider {
        fn new() -> Self {
            Self {
                send_count: AtomicU64::new(0),
                check_count: AtomicU64::new(0),
            }
        }

        fn send_count(&self) -> u64 {
            self.send_count.load(Ordering::SeqCst)
        }
    }

    impl HeartbeatProvider for CountingHeartbeatProvider {
        fn send_heartbeat(&self, _node: &ClusterNode) -> Result<(), ClusterError> {
            self.send_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn check_node_alive(&self, node: &ClusterNode) -> Result<bool, ClusterError> {
            self.check_count.fetch_add(1, Ordering::SeqCst);
            Ok(node.is_alive())
        }
    }

    /// 记录型任务分发器（记录 dispatch 调用）
    struct RecordingTaskDispatcher {
        dispatch_log: Mutex<Vec<(String, String, String)>>,
        migrate_log: Mutex<Vec<(String, String, String)>>,
    }

    impl RecordingTaskDispatcher {
        fn new() -> Self {
            Self {
                dispatch_log: Mutex::new(Vec::new()),
                migrate_log: Mutex::new(Vec::new()),
            }
        }

        fn dispatch_log(&self) -> Vec<(String, String, String)> {
            self.dispatch_log.lock().clone()
        }

        fn migrate_log(&self) -> Vec<(String, String, String)> {
            self.migrate_log.lock().clone()
        }
    }

    impl TaskDispatcher for RecordingTaskDispatcher {
        fn dispatch_task(
            &self,
            task_id: &str,
            source_id: &str,
            target_node: &ClusterNode,
        ) -> Result<(), ClusterError> {
            self.dispatch_log.lock().push((
                task_id.to_string(),
                source_id.to_string(),
                target_node.node_id.clone(),
            ));
            Ok(())
        }

        fn undispatch_task(&self, _task_id: &str, _node: &ClusterNode) -> Result<(), ClusterError> {
            Ok(())
        }

        fn migrate_task(
            &self,
            task_id: &str,
            source_node: &ClusterNode,
            target_node: &ClusterNode,
        ) -> Result<(), ClusterError> {
            self.migrate_log.lock().push((
                task_id.to_string(),
                source_node.node_id.clone(),
                target_node.node_id.clone(),
            ));
            Ok(())
        }
    }

    #[test]
    fn injected_task_dispatcher_records_calls() {
        let dispatcher = Arc::new(RecordingTaskDispatcher::new());
        let coord = ClusterCoordinator::with_timestamp_fn(
            ClusterConfig::default(),
            Box::new(|| 1000),
            Arc::new(NoopHeartbeatProvider),
            dispatcher.clone(),
        );
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.assign_task("task-1", "source-1").unwrap();
        let log = dispatcher.dispatch_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, "task-1");
        assert_eq!(log[0].1, "source-1");
        assert_eq!(log[0].2, "node-1");
    }

    #[test]
    fn injected_task_dispatcher_records_migration() {
        let dispatcher = Arc::new(RecordingTaskDispatcher::new());
        let coord = ClusterCoordinator::with_timestamp_fn(
            ClusterConfig::default(),
            Box::new(|| 1000),
            Arc::new(NoopHeartbeatProvider),
            dispatcher.clone(),
        );
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        coord.assign_task("task-1", "").unwrap();
        coord.migrate_task("task-1", "node-2").unwrap();
        let log = dispatcher.migrate_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, "task-1");
        assert_eq!(log[0].1, "node-1");
        assert_eq!(log[0].2, "node-2");
    }

    #[test]
    fn injected_heartbeat_provider_can_be_used() {
        let provider = Arc::new(CountingHeartbeatProvider::new());
        let coord = ClusterCoordinator::with_timestamp_fn(
            ClusterConfig::default(),
            Box::new(|| 1000),
            provider.clone(),
            Arc::new(NoopTaskDispatcher),
        );
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        let node = coord.get_node("node-1").unwrap();
        let _ = provider.send_heartbeat(&node);
        assert_eq!(provider.send_count(), 1);
    }

    // =================================================================
    // 14. 边界场景测试
    // =================================================================

    #[test]
    fn empty_cluster_has_no_leader() {
        let coord = make_test_coordinator(ClusterConfig::default());
        assert!(coord.current_leader().is_none());
        assert_eq!(coord.node_count(), 0);
        assert_eq!(coord.assignment_count(), 0);
    }

    #[test]
    fn affinity_persists_after_unassign() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        coord.assign_task("task-1", "source-1").unwrap();
        let first_node = coord.get_assignment("task-1").unwrap();
        coord.unassign_task("task-1").unwrap();
        // 亲和性索引保留，新任务同源应分配到同节点
        coord.assign_task("task-2", "source-1").unwrap();
        let second_node = coord.get_assignment("task-2").unwrap();
        assert_eq!(first_node, second_node);
    }

    #[test]
    fn migrate_task_updates_affinity_index() {
        let coord = make_test_coordinator(ClusterConfig::default());
        register_three_nodes(&coord);
        coord.assign_task("task-1", "source-1").unwrap();
        let first_node = coord.get_assignment("task-1").unwrap();
        coord.migrate_task("task-1", "node-2").unwrap();
        // 亲和性应更新到 node-2
        assert_eq!(
            coord.get_source_affinity("source-1"),
            Some("node-2".to_string())
        );
        // 新同源任务应分配到 node-2（亲和性）
        coord.assign_task("task-2", "source-1").unwrap();
        assert_eq!(coord.get_assignment("task-2"), Some("node-2".to_string()));
        let _ = first_node; // 抑制 unused warning
    }

    #[test]
    fn unregister_leader_triggers_reelection() {
        let coord = make_test_coordinator(ClusterConfig::default());
        coord.register_node("node-1", "10.0.0.1:8080", 10).unwrap();
        coord.register_node("node-2", "10.0.0.2:8080", 10).unwrap();
        assert_eq!(coord.current_leader(), Some("node-1".to_string()));
        coord.unregister_node("node-1").unwrap();
        // node-1 下线后，node-2 应成为新 Leader
        assert_eq!(coord.current_leader(), Some("node-2".to_string()));
    }

    #[test]
    fn node_capacity_default_is_10() {
        let cap = NodeCapacity::default();
        assert_eq!(cap.max_tasks, 10);
        assert_eq!(cap.current_tasks, 0);
    }

    #[test]
    fn cluster_coordinator_config_returns_reference() {
        let cfg = ClusterConfig::default().with_max_tasks_per_node(7);
        let coord = make_test_coordinator(cfg);
        assert_eq!(coord.config().max_tasks_per_node, 7);
    }

    #[test]
    fn task_assignment_select_node_returns_none_for_empty() {
        let nodes: Vec<ClusterNode> = Vec::new();
        let result = TaskAssignment::select_node(&nodes, None);
        assert!(result.is_none());
    }

    #[test]
    fn task_assignment_select_node_with_affinity() {
        let n1 = ClusterNode::new("node-1", "10.0.0.1:8080", 10);
        let n2 = ClusterNode::new("node-2", "10.0.0.2:8080", 10);
        let nodes = vec![n1.clone(), n2.clone()];
        // 指定亲和 node-2，应返回 node-2（即使 node-1 字典序更小）
        let selected = TaskAssignment::select_node(&nodes, Some("node-2")).unwrap();
        assert_eq!(selected.node_id, "node-2");
    }

    #[test]
    fn task_assignment_select_node_falls_back_when_affinity_not_in_list() {
        let n1 = ClusterNode::new("node-1", "10.0.0.1:8080", 10);
        let n2 = ClusterNode::new("node-2", "10.0.0.2:8080", 10);
        let nodes = vec![n1, n2];
        // 亲和节点 node-x 不在列表中，应回退到负载均衡选 node-1
        let selected = TaskAssignment::select_node(&nodes, Some("node-x")).unwrap();
        assert_eq!(selected.node_id, "node-1");
    }

    #[test]
    fn task_assignment_filter_available_excludes_full_nodes() {
        let mut n1 = ClusterNode::new("node-1", "10.0.0.1:8080", 1);
        n1.capacity.current_tasks = 1; // 已满
        let n2 = ClusterNode::new("node-2", "10.0.0.2:8080", 10);
        let nodes = vec![n1, n2];
        let available = TaskAssignment::filter_available(&nodes);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].node_id, "node-2");
    }
}
