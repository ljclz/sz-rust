//! 灰度升级（Rolling Upgrade）— Phase 7e.5
//!
//! 对应 `SzRSQL实施进度.md` Phase 7e.5。
//!
//! # 设计目标
//!
//! - 多节点集群逐个节点升级，升级期间业务不中断
//! - 升级顺序：先 Follower（逐个）→ 最后 Leader（切换后再升级）
//! - 灰度升级仅支持 PATCH/MINOR（major 相同），MAJOR 升级需走 Phase 7e.3
//!
//! # 流程
//!
//! ```text
//! 初始：Leader(N1, v1.0.0) + Follower(N2, v1.0.0) + Follower(N3, v1.0.0)
//!   ↓
//! Step 1: 升级 Follower N2 → v1.0.1（N1 Leader 不变，N3 仍可读）
//!   ↓
//! Step 2: 升级 Follower N3 → v1.0.1（N1 Leader 不变，N2 已是新版本可读）
//!   ↓
//! Step 3: 切换 Leader N1 → N2，升级旧 Leader N1 → v1.0.1（N2 新 Leader，N3 可读）
//!   ↓
//! 最终：Follower(N1, v1.0.1) + Leader(N2, v1.0.1) + Follower(N3, v1.0.1)
//! ```
//!
//! # 验证标准
//!
//! - 3 节点集群逐个升级，每一步 `writable && readable` 均为 true
//! - 升级后所有节点版本一致
//! - Leader 切换正确（旧 Leader 成为 Follower）

use serde::{Deserialize, Serialize};
use std::fmt;

// =====================================================================
//  NodeVersion — 节点版本号（轻量 SemVer）
// =====================================================================

/// 节点版本号（轻量 SemVer，仅 major.minor.patch）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeVersion {
    /// 主版本号
    pub major: u32,
    /// 次版本号
    pub minor: u32,
    /// 修订号
    pub patch: u32,
}

impl NodeVersion {
    /// 创建新版本号
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// 解析版本号字符串（"1.0.0"）
    pub fn parse(s: &str) -> Result<Self, RollingError> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(RollingError::InvalidVersion(format!(
                "expected MAJOR.MINOR.PATCH, got '{}'",
                s
            )));
        }
        let major = parts[0].parse().map_err(|_| {
            RollingError::InvalidVersion(format!("invalid major '{}' in '{}'", parts[0], s))
        })?;
        let minor = parts[1].parse().map_err(|_| {
            RollingError::InvalidVersion(format!("invalid minor '{}' in '{}'", parts[1], s))
        })?;
        let patch = parts[2].parse().map_err(|_| {
            RollingError::InvalidVersion(format!("invalid patch '{}' in '{}'", parts[2], s))
        })?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// 当前版本是否小于目标版本（用于校验升级方向）
    pub fn is_upgrade_to(&self, target: &Self) -> bool {
        let self_tuple = (self.major, self.minor, self.patch);
        let target_tuple = (target.major, target.minor, target.patch);
        self_tuple < target_tuple
    }

    /// 是否与目标版本兼容（major 相同，灰度升级期间可混合运行）
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

impl fmt::Display for NodeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for NodeVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NodeVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

// =====================================================================
//  NodeRole / NodeState / NodeInfo — 节点状态
// =====================================================================

/// 节点角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeRole {
    /// 主节点
    Leader,
    /// 从节点
    Follower,
}

impl NodeRole {
    /// 返回角色名称
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeRole::Leader => "leader",
            NodeRole::Follower => "follower",
        }
    }
}

impl fmt::Display for NodeRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 节点运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeState {
    /// 正常运行
    Running,
    /// 升级中（暂不可用）
    Upgrading,
    /// 已升级（新版本，可用）
    Upgraded,
    /// 下线
    Down,
}

impl NodeState {
    /// 是否可用（可参与读写）
    pub fn is_available(&self) -> bool {
        matches!(self, NodeState::Running | NodeState::Upgraded)
    }

    /// 返回状态名称
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeState::Running => "running",
            NodeState::Upgrading => "upgrading",
            NodeState::Upgraded => "upgraded",
            NodeState::Down => "down",
        }
    }
}

impl fmt::Display for NodeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 节点信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    /// 节点 ID（唯一）
    pub node_id: u32,
    /// 节点角色
    pub role: NodeRole,
    /// 运行状态
    pub state: NodeState,
    /// 当前版本
    pub version: NodeVersion,
}

impl NodeInfo {
    /// 创建新节点
    pub fn new(node_id: u32, role: NodeRole, version: NodeVersion) -> Self {
        Self {
            node_id,
            role,
            state: NodeState::Running,
            version,
        }
    }

    /// 是否可用
    pub fn is_available(&self) -> bool {
        self.state.is_available()
    }
}

// =====================================================================
//  Cluster — 集群状态
// =====================================================================

/// 集群状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cluster {
    /// 节点列表
    pub nodes: Vec<NodeInfo>,
    /// 当前 Leader 节点 ID
    pub leader_id: u32,
}

impl Cluster {
    /// 创建新集群
    pub fn new(nodes: Vec<NodeInfo>, leader_id: u32) -> Self {
        Self { nodes, leader_id }
    }

    /// 创建 3 节点集群（1 Leader + 2 Follower），所有节点版本相同
    pub fn three_node_uniform(version: NodeVersion) -> Self {
        let nodes = vec![
            NodeInfo::new(1, NodeRole::Leader, version),
            NodeInfo::new(2, NodeRole::Follower, version),
            NodeInfo::new(3, NodeRole::Follower, version),
        ];
        Self::new(nodes, 1)
    }

    /// 创建 N 节点集群（1 Leader + (N-1) Follower），所有节点版本相同
    pub fn n_node_uniform(node_count: u32, version: NodeVersion) -> Self {
        let nodes = (1..=node_count)
            .map(|id| {
                let role = if id == 1 {
                    NodeRole::Leader
                } else {
                    NodeRole::Follower
                };
                NodeInfo::new(id, role, version)
            })
            .collect();
        Self::new(nodes, 1)
    }

    /// 获取指定节点
    pub fn get_node(&self, node_id: u32) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.node_id == node_id)
    }

    /// 获取指定节点的可变引用
    pub fn get_node_mut(&mut self, node_id: u32) -> Option<&mut NodeInfo> {
        self.nodes.iter_mut().find(|n| n.node_id == node_id)
    }

    /// 获取 Leader 节点
    pub fn leader(&self) -> Option<&NodeInfo> {
        self.get_node(self.leader_id)
    }

    /// 获取所有 Follower 节点
    pub fn followers(&self) -> Vec<&NodeInfo> {
        self.nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower)
            .collect()
    }

    /// 获取所有可用节点
    pub fn available_nodes(&self) -> Vec<&NodeInfo> {
        self.nodes.iter().filter(|n| n.is_available()).collect()
    }

    /// 获取所有可用 Follower
    pub fn available_followers(&self) -> Vec<&NodeInfo> {
        self.nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower && n.is_available())
            .collect()
    }

    /// Leader 是否可用
    pub fn leader_available(&self) -> bool {
        self.leader().map(|n| n.is_available()).unwrap_or(false)
    }

    /// 集群可用性
    pub fn availability(&self) -> ClusterAvailability {
        let total_nodes = self.nodes.len();
        let running_nodes = self.available_nodes().len();
        let follower_count_available = self.available_followers().len();
        let leader_available = self.leader_available();
        ClusterAvailability {
            leader_available,
            follower_count_available,
            total_nodes,
            running_nodes,
            writable: leader_available,
            readable: running_nodes > 0,
        }
    }

    /// 校验所有节点版本一致
    pub fn all_versions_uniform(&self) -> bool {
        if self.nodes.is_empty() {
            return true;
        }
        let v = self.nodes[0].version;
        self.nodes.iter().all(|n| n.version == v)
    }

    /// 校验所有节点版本与指定版本兼容（major 相同）
    pub fn all_compatible_with(&self, target: &NodeVersion) -> bool {
        self.nodes
            .iter()
            .all(|n| n.version.is_compatible_with(target))
    }
}

// =====================================================================
//  ClusterAvailability — 集群可用性
// =====================================================================

/// 集群可用性快照
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterAvailability {
    /// Leader 是否可用
    pub leader_available: bool,
    /// 可用 Follower 数量
    pub follower_count_available: usize,
    /// 节点总数
    pub total_nodes: usize,
    /// 可用节点数
    pub running_nodes: usize,
    /// 是否可写（Leader 可用）
    pub writable: bool,
    /// 是否可读（至少 1 个节点可用）
    pub readable: bool,
}

impl ClusterAvailability {
    /// 业务是否不中断（可写且可读）
    pub fn no_outage(&self) -> bool {
        self.writable && self.readable
    }
}

impl fmt::Display for ClusterAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "availability(leader={}, followers={}/{}, nodes={}/{}, writable={}, readable={})",
            self.leader_available,
            self.follower_count_available,
            self.total_nodes.saturating_sub(1),
            self.running_nodes,
            self.total_nodes,
            self.writable,
            self.readable
        )
    }
}

// =====================================================================
//  RollingError — 灰度升级错误
// =====================================================================

/// 灰度升级错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RollingError {
    /// 无效的版本号
    #[error("invalid version: {0}")]
    InvalidVersion(String),

    /// 版本不兼容（灰度升级要求 major 相同）
    #[error(
        "version incompatible: from {from} to {to}, rolling upgrade requires same major version"
    )]
    IncompatibleVersion {
        /// 源版本
        from: NodeVersion,
        /// 目标版本
        to: NodeVersion,
    },

    /// 无可用 Follower
    #[error("no follower available")]
    NoFollowerAvailable,

    /// 无可用 Leader
    #[error("no leader available")]
    NoLeaderAvailable,

    /// 节点不存在
    #[error("node {0} not found")]
    NodeNotFound(u32),

    /// 节点未运行
    #[error("node {0} is not running (state: {1})")]
    NodeNotRunning(u32, NodeState),

    /// 升级顺序违规
    #[error("upgrade order violation: expected node {expected}, tried node {actual}")]
    UpgradeOrderViolation {
        /// 期望的节点 ID
        expected: u32,
        /// 实际尝试的节点 ID
        actual: u32,
    },

    /// 升级已完成
    #[error("rolling upgrade already complete")]
    UpgradeAlreadyComplete,

    /// 升级步骤失败
    #[error("upgrade step failed for node {node_id}: {reason}")]
    StepFailed {
        /// 失败节点 ID
        node_id: u32,
        /// 失败原因
        reason: String,
    },

    /// 集群版本不一致
    #[error("cluster nodes have inconsistent versions")]
    InconsistentVersions,

    /// 无新版本可用（目标版本不大于当前版本）
    #[error("target version {target} is not an upgrade from current {current}")]
    NotAnUpgrade {
        /// 当前版本
        current: NodeVersion,
        /// 目标版本
        target: NodeVersion,
    },
}

// =====================================================================
//  RollingStep — 单步升级记录
// =====================================================================

/// 单步升级记录
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollingStep {
    /// 步骤索引（0-based）
    pub step_index: usize,
    /// 升级的节点 ID
    pub node_id: u32,
    /// 升级前版本
    pub from_version: NodeVersion,
    /// 升级后版本
    pub to_version: NodeVersion,
    /// 升级时节点角色
    pub role_at_upgrade: NodeRole,
    /// 升级前 Leader ID
    pub leader_before: u32,
    /// 升级后 Leader ID（Leader 切换时变化）
    pub leader_after: u32,
    /// 升级前集群可用性
    pub availability_before: ClusterAvailability,
    /// 升级后集群可用性
    pub availability_after: ClusterAvailability,
    /// 是否成功
    pub success: bool,
    /// 附加消息
    pub message: String,
}

// =====================================================================
//  RollingUpgradePlanner — 灰度升级规划器
// =====================================================================

/// 灰度升级规划器
pub struct RollingUpgradePlanner;

impl RollingUpgradePlanner {
    /// 规划升级顺序
    ///
    /// 返回节点 ID 升级顺序：先 Follower（按 ID 升序），最后 Leader。
    ///
    /// 校验：
    /// - 所有节点版本一致
    /// - 所有节点版本与目标版本兼容（major 相同）
    /// - 目标版本是升级（大于当前版本）
    /// - 至少 1 个 Follower 可用
    /// - Leader 可用
    pub fn plan_upgrade_order(
        cluster: &Cluster,
        target_version: &NodeVersion,
    ) -> Result<Vec<u32>, RollingError> {
        // 1. 校验所有节点版本一致
        if !cluster.all_versions_uniform() {
            return Err(RollingError::InconsistentVersions);
        }

        // 2. 校验版本兼容（major 相同）
        let current_version = cluster
            .nodes
            .first()
            .map(|n| n.version)
            .ok_or(RollingError::NoFollowerAvailable)?;
        if !current_version.is_compatible_with(target_version) {
            return Err(RollingError::IncompatibleVersion {
                from: current_version,
                to: *target_version,
            });
        }

        // 3. 校验是升级（目标版本 > 当前版本）
        if !current_version.is_upgrade_to(target_version) {
            return Err(RollingError::NotAnUpgrade {
                current: current_version,
                target: *target_version,
            });
        }

        // 4. 校验至少 1 个 Follower 可用
        if cluster.available_followers().is_empty() {
            return Err(RollingError::NoFollowerAvailable);
        }

        // 5. 校验 Leader 可用
        if !cluster.leader_available() {
            return Err(RollingError::NoLeaderAvailable);
        }

        // 6. 规划顺序：Follower（按 ID 升序）→ Leader
        let mut followers: Vec<u32> = cluster
            .nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower && n.is_available())
            .map(|n| n.node_id)
            .collect();
        followers.sort();
        let mut order = followers;
        order.push(cluster.leader_id);
        Ok(order)
    }
}

// =====================================================================
//  RollingUpgradeExecutor — 灰度升级执行器
// =====================================================================

/// 灰度升级执行器
///
/// 按规划顺序逐个节点升级，每一步保证集群可用性。
/// Leader 升级前先切换 Leader 到已升级的 Follower。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollingUpgradeExecutor {
    /// 集群状态
    pub cluster: Cluster,
    /// 目标版本
    pub target_version: NodeVersion,
    /// 升级顺序（节点 ID 列表）
    pub upgrade_order: Vec<u32>,
    /// 当前步骤索引
    pub current_step: usize,
    /// 已完成的步骤记录
    pub steps_log: Vec<RollingStep>,
}

impl RollingUpgradeExecutor {
    /// 创建灰度升级执行器
    pub fn new(cluster: Cluster, target_version: NodeVersion) -> Result<Self, RollingError> {
        let upgrade_order = RollingUpgradePlanner::plan_upgrade_order(&cluster, &target_version)?;
        Ok(Self {
            cluster,
            target_version,
            upgrade_order,
            current_step: 0,
            steps_log: Vec::new(),
        })
    }

    /// 是否已完成所有升级步骤
    pub fn is_complete(&self) -> bool {
        self.current_step >= self.upgrade_order.len()
    }

    /// 剩余步骤数
    pub fn remaining_steps(&self) -> usize {
        self.upgrade_order.len().saturating_sub(self.current_step)
    }

    /// 总步骤数
    pub fn total_steps(&self) -> usize {
        self.upgrade_order.len()
    }

    /// 当前待升级节点 ID
    pub fn current_node_id(&self) -> Option<u32> {
        self.upgrade_order.get(self.current_step).copied()
    }

    /// 执行下一步升级
    ///
    /// 返回步骤记录。如果是 Leader 升级，会先切换 Leader。
    pub fn execute_next_step(&mut self) -> Result<RollingStep, RollingError> {
        if self.is_complete() {
            return Err(RollingError::UpgradeAlreadyComplete);
        }

        let node_id = self.current_node_id().unwrap();
        let node = self
            .cluster
            .get_node(node_id)
            .ok_or(RollingError::NodeNotFound(node_id))?
            .clone();

        if !node.is_available() {
            return Err(RollingError::NodeNotRunning(node_id, node.state));
        }

        let from_version = node.version;
        let role_at_upgrade = node.role;
        let leader_before = self.cluster.leader_id;
        let availability_before = self.cluster.availability();

        // 校验升级前集群可用
        if !availability_before.no_outage() {
            return Err(RollingError::StepFailed {
                node_id,
                reason: format!("cluster outage before upgrade: {}", availability_before),
            });
        }

        let mut message = String::new();
        let mut leader_after = leader_before;

        // 如果是 Leader 升级，先切换 Leader
        if role_at_upgrade == NodeRole::Leader {
            leader_after = self.elect_new_leader(node_id)?;
            message.push_str(&format!(
                "leader switched {} → {}; ",
                leader_before, leader_after
            ));
        }

        // 标记节点为 Upgrading
        self.set_node_state(node_id, NodeState::Upgrading);

        // 校验升级中集群仍可用（Leader 切换后）
        let during_availability = self.cluster.availability();
        if !during_availability.no_outage() {
            // 回滚状态
            self.set_node_state(node_id, NodeState::Running);
            return Err(RollingError::StepFailed {
                node_id,
                reason: format!("cluster outage during upgrade: {}", during_availability),
            });
        }

        // 执行升级（修改版本号）
        self.upgrade_node_version(node_id, self.target_version)?;

        // 标记节点为 Upgraded（已升级，可用）
        self.set_node_state(node_id, NodeState::Upgraded);

        let availability_after = self.cluster.availability();
        if !availability_after.no_outage() {
            return Err(RollingError::StepFailed {
                node_id,
                reason: format!("cluster outage after upgrade: {}", availability_after),
            });
        }

        message.push_str(&format!(
            "node {} upgraded {} → {}",
            node_id, from_version, self.target_version
        ));

        let step = RollingStep {
            step_index: self.current_step,
            node_id,
            from_version,
            to_version: self.target_version,
            role_at_upgrade,
            leader_before,
            leader_after,
            availability_before,
            availability_after,
            success: true,
            message,
        };

        self.steps_log.push(step.clone());
        self.current_step += 1;
        Ok(step)
    }

    /// 执行所有剩余步骤
    pub fn execute_all(&mut self) -> Result<Vec<RollingStep>, RollingError> {
        let mut steps = Vec::new();
        while !self.is_complete() {
            steps.push(self.execute_next_step()?);
        }
        Ok(steps)
    }

    /// 选举新 Leader（从已升级的 Follower 中选）
    ///
    /// 参数 `excluding_node_id` 是即将升级的旧 Leader，排除。
    fn elect_new_leader(&mut self, excluding_node_id: u32) -> Result<u32, RollingError> {
        // 优先选已升级（Upgraded 状态）的 Follower
        let candidates: Vec<u32> = self
            .cluster
            .nodes
            .iter()
            .filter(|n| {
                n.node_id != excluding_node_id
                    && n.role == NodeRole::Follower
                    && n.state == NodeState::Upgraded
            })
            .map(|n| n.node_id)
            .collect();

        let new_leader_id =
            candidates
                .first()
                .copied()
                .ok_or_else(|| RollingError::StepFailed {
                    node_id: excluding_node_id,
                    reason: "no upgraded follower available for leader switch".to_string(),
                })?;

        // 切换角色：旧 Leader → Follower，新 Leader ← Follower
        if let Some(old_leader) = self.cluster.get_node_mut(excluding_node_id) {
            old_leader.role = NodeRole::Follower;
        }
        if let Some(new_leader) = self.cluster.get_node_mut(new_leader_id) {
            new_leader.role = NodeRole::Leader;
        }
        self.cluster.leader_id = new_leader_id;

        Ok(new_leader_id)
    }

    /// 设置节点状态
    fn set_node_state(&mut self, node_id: u32, state: NodeState) {
        if let Some(node) = self.cluster.get_node_mut(node_id) {
            node.state = state;
        }
    }

    /// 升级节点版本
    fn upgrade_node_version(
        &mut self,
        node_id: u32,
        new_version: NodeVersion,
    ) -> Result<(), RollingError> {
        let node = self
            .cluster
            .get_node_mut(node_id)
            .ok_or(RollingError::NodeNotFound(node_id))?;
        node.version = new_version;
        Ok(())
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_version_new() {
        let v = NodeVersion::new(1, 0, 0);
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
    }

    #[test]
    fn node_version_parse_simple() {
        let v = NodeVersion::parse("1.0.0").unwrap();
        assert_eq!(v, NodeVersion::new(1, 0, 0));
    }

    #[test]
    fn node_version_parse_complex() {
        let v = NodeVersion::parse("2.5.17").unwrap();
        assert_eq!(v, NodeVersion::new(2, 5, 17));
    }

    #[test]
    fn node_version_parse_invalid_fails() {
        assert!(NodeVersion::parse("1.0").is_err());
        assert!(NodeVersion::parse("1.0.0.0").is_err());
        assert!(NodeVersion::parse("abc").is_err());
        assert!(NodeVersion::parse("1.x.0").is_err());
        assert!(NodeVersion::parse("").is_err());
    }

    #[test]
    fn node_version_is_upgrade_to() {
        let v1 = NodeVersion::new(1, 0, 0);
        let v2 = NodeVersion::new(1, 0, 1);
        let v3 = NodeVersion::new(1, 1, 0);
        let v4 = NodeVersion::new(2, 0, 0);
        assert!(v1.is_upgrade_to(&v2));
        assert!(v1.is_upgrade_to(&v3));
        assert!(v1.is_upgrade_to(&v4));
        assert!(!v2.is_upgrade_to(&v1));
        assert!(!v1.is_upgrade_to(&v1));
    }

    #[test]
    fn node_version_is_compatible_with() {
        let v1 = NodeVersion::new(1, 0, 0);
        let v2 = NodeVersion::new(1, 5, 3);
        let v3 = NodeVersion::new(2, 0, 0);
        assert!(v1.is_compatible_with(&v2));
        assert!(v2.is_compatible_with(&v1));
        assert!(!v1.is_compatible_with(&v3));
    }

    #[test]
    fn node_version_display() {
        let v = NodeVersion::new(1, 2, 3);
        assert_eq!(format!("{}", v), "1.2.3");
    }

    #[test]
    fn node_version_ordering() {
        let v1 = NodeVersion::new(1, 0, 0);
        let v2 = NodeVersion::new(1, 0, 1);
        let v3 = NodeVersion::new(1, 1, 0);
        let v4 = NodeVersion::new(2, 0, 0);
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert!(v4 > v1);
    }

    #[test]
    fn node_role_as_str() {
        assert_eq!(NodeRole::Leader.as_str(), "leader");
        assert_eq!(NodeRole::Follower.as_str(), "follower");
    }

    #[test]
    fn node_state_is_available() {
        assert!(NodeState::Running.is_available());
        assert!(NodeState::Upgraded.is_available());
        assert!(!NodeState::Upgrading.is_available());
        assert!(!NodeState::Down.is_available());
    }

    #[test]
    fn node_info_new_defaults_to_running() {
        let n = NodeInfo::new(1, NodeRole::Leader, NodeVersion::new(1, 0, 0));
        assert_eq!(n.node_id, 1);
        assert_eq!(n.role, NodeRole::Leader);
        assert_eq!(n.state, NodeState::Running);
        assert!(n.is_available());
    }

    #[test]
    fn cluster_three_node_uniform_setup() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.leader_id, 1);
        assert_eq!(c.nodes[0].role, NodeRole::Leader);
        assert_eq!(c.nodes[1].role, NodeRole::Follower);
        assert_eq!(c.nodes[2].role, NodeRole::Follower);
    }

    #[test]
    fn cluster_n_node_uniform_setup() {
        let c = Cluster::n_node_uniform(5, NodeVersion::new(1, 0, 0));
        assert_eq!(c.nodes.len(), 5);
        assert_eq!(c.leader_id, 1);
        assert_eq!(c.nodes[0].role, NodeRole::Leader);
        for n in &c.nodes[1..] {
            assert_eq!(n.role, NodeRole::Follower);
        }
    }

    #[test]
    fn cluster_get_node() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        assert!(c.get_node(1).is_some());
        assert!(c.get_node(2).is_some());
        assert!(c.get_node(3).is_some());
        assert!(c.get_node(99).is_none());
    }

    #[test]
    fn cluster_leader_and_followers() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        assert_eq!(c.leader().unwrap().node_id, 1);
        assert_eq!(c.followers().len(), 2);
    }

    #[test]
    fn cluster_availability_all_running() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let avail = c.availability();
        assert!(avail.leader_available);
        assert_eq!(avail.follower_count_available, 2);
        assert_eq!(avail.total_nodes, 3);
        assert_eq!(avail.running_nodes, 3);
        assert!(avail.writable);
        assert!(avail.readable);
        assert!(avail.no_outage());
    }

    #[test]
    fn cluster_availability_one_follower_down() {
        let mut c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        c.get_node_mut(2).unwrap().state = NodeState::Down;
        let avail = c.availability();
        assert!(avail.leader_available);
        assert_eq!(avail.follower_count_available, 1);
        assert_eq!(avail.running_nodes, 2);
        assert!(avail.no_outage());
    }

    #[test]
    fn cluster_availability_leader_down() {
        let mut c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        c.get_node_mut(1).unwrap().state = NodeState::Down;
        let avail = c.availability();
        assert!(!avail.leader_available);
        assert!(!avail.writable);
        assert!(avail.readable);
        assert!(!avail.no_outage());
    }

    #[test]
    fn cluster_all_versions_uniform() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        assert!(c.all_versions_uniform());
        let mut c2 = c.clone();
        c2.get_node_mut(2).unwrap().version = NodeVersion::new(1, 0, 1);
        assert!(!c2.all_versions_uniform());
    }

    #[test]
    fn cluster_all_compatible_with() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        assert!(c.all_compatible_with(&NodeVersion::new(1, 5, 3)));
        assert!(!c.all_compatible_with(&NodeVersion::new(2, 0, 0)));
    }

    #[test]
    fn planner_three_node_order_followers_then_leader() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let order =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 0, 1)).unwrap();
        assert_eq!(order, vec![2, 3, 1]);
    }

    #[test]
    fn planner_five_node_order() {
        let c = Cluster::n_node_uniform(5, NodeVersion::new(1, 0, 0));
        let order =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 1, 0)).unwrap();
        assert_eq!(order, vec![2, 3, 4, 5, 1]);
    }

    #[test]
    fn planner_rejects_major_upgrade() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let err =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(2, 0, 0)).unwrap_err();
        assert!(matches!(err, RollingError::IncompatibleVersion { .. }));
    }

    #[test]
    fn planner_rejects_downgrade() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 1, 0));
        let err =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 0, 0)).unwrap_err();
        assert!(matches!(err, RollingError::NotAnUpgrade { .. }));
    }

    #[test]
    fn planner_rejects_same_version() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let err =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 0, 0)).unwrap_err();
        assert!(matches!(err, RollingError::NotAnUpgrade { .. }));
    }

    #[test]
    fn planner_rejects_inconsistent_versions() {
        let mut c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        c.get_node_mut(2).unwrap().version = NodeVersion::new(1, 0, 1);
        let err =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 0, 2)).unwrap_err();
        assert!(matches!(err, RollingError::InconsistentVersions));
    }

    #[test]
    fn planner_rejects_no_follower_available() {
        let mut c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        c.get_node_mut(2).unwrap().state = NodeState::Down;
        c.get_node_mut(3).unwrap().state = NodeState::Down;
        let err =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 0, 1)).unwrap_err();
        assert!(matches!(err, RollingError::NoFollowerAvailable));
    }

    #[test]
    fn planner_rejects_leader_down() {
        let mut c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        c.get_node_mut(1).unwrap().state = NodeState::Down;
        let err =
            RollingUpgradePlanner::plan_upgrade_order(&c, &NodeVersion::new(1, 0, 1)).unwrap_err();
        assert!(matches!(err, RollingError::NoLeaderAvailable));
    }

    #[test]
    fn executor_three_node_patch_upgrade_zero_outage() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        assert_eq!(exec.total_steps(), 3);
        assert_eq!(exec.remaining_steps(), 3);
        assert!(!exec.is_complete());

        let step1 = exec.execute_next_step().unwrap();
        assert_eq!(step1.node_id, 2);
        assert_eq!(step1.role_at_upgrade, NodeRole::Follower);
        assert_eq!(step1.leader_before, 1);
        assert_eq!(step1.leader_after, 1);
        assert_eq!(step1.from_version, NodeVersion::new(1, 0, 0));
        assert_eq!(step1.to_version, NodeVersion::new(1, 0, 1));
        assert!(step1.availability_before.no_outage());
        assert!(step1.availability_after.no_outage());
        assert_eq!(exec.remaining_steps(), 2);

        let step2 = exec.execute_next_step().unwrap();
        assert_eq!(step2.node_id, 3);
        assert_eq!(step2.role_at_upgrade, NodeRole::Follower);
        assert_eq!(step2.leader_before, 1);
        assert_eq!(step2.leader_after, 1);
        assert!(step2.availability_after.no_outage());

        let step3 = exec.execute_next_step().unwrap();
        assert_eq!(step3.node_id, 1);
        assert_eq!(step3.role_at_upgrade, NodeRole::Leader);
        assert_eq!(step3.leader_before, 1);
        assert_eq!(step3.leader_after, 2);
        assert!(step3.availability_after.no_outage());

        assert!(exec.is_complete());
        assert_eq!(exec.remaining_steps(), 0);
        assert!(exec.cluster.all_versions_uniform());
        assert_eq!(exec.cluster.nodes[0].version, NodeVersion::new(1, 0, 1));
        assert_eq!(exec.cluster.leader_id, 2);
        assert_eq!(exec.cluster.get_node(1).unwrap().role, NodeRole::Follower);
        assert_eq!(exec.cluster.get_node(2).unwrap().role, NodeRole::Leader);
    }

    #[test]
    fn executor_minor_upgrade_zero_outage() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 1, 0)).unwrap();
        let steps = exec.execute_all().unwrap();
        assert_eq!(steps.len(), 3);
        for step in &steps {
            assert!(step.availability_before.no_outage());
            assert!(step.availability_after.no_outage());
        }
        assert!(exec.is_complete());
        assert!(exec.cluster.all_versions_uniform());
        assert_eq!(exec.cluster.nodes[0].version, NodeVersion::new(1, 1, 0));
    }

    #[test]
    fn executor_five_node_upgrade_zero_outage() {
        let cluster = Cluster::n_node_uniform(5, NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 5)).unwrap();
        assert_eq!(exec.total_steps(), 5);
        let steps = exec.execute_all().unwrap();
        assert_eq!(steps.len(), 5);
        for step in &steps {
            assert!(step.availability_after.no_outage());
        }
        assert!(exec.cluster.all_versions_uniform());
        assert_eq!(exec.cluster.nodes[0].version, NodeVersion::new(1, 0, 5));
    }

    #[test]
    fn executor_complete_returns_already_complete_error() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        exec.execute_all().unwrap();
        assert!(exec.is_complete());
        let err = exec.execute_next_step().unwrap_err();
        assert!(matches!(err, RollingError::UpgradeAlreadyComplete));
    }

    #[test]
    fn executor_leader_switch_to_first_upgraded_follower() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        exec.execute_next_step().unwrap();
        exec.execute_next_step().unwrap();
        let step3 = exec.execute_next_step().unwrap();
        assert_eq!(step3.leader_after, 2);
        assert_eq!(exec.cluster.leader_id, 2);
        assert_eq!(exec.cluster.get_node(1).unwrap().role, NodeRole::Follower);
        assert_eq!(exec.cluster.get_node(2).unwrap().role, NodeRole::Leader);
        assert_eq!(exec.cluster.get_node(3).unwrap().role, NodeRole::Follower);
    }

    #[test]
    fn executor_rejects_major_upgrade_at_init() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let err = RollingUpgradeExecutor::new(cluster, NodeVersion::new(2, 0, 0)).unwrap_err();
        assert!(matches!(err, RollingError::IncompatibleVersion { .. }));
    }

    #[test]
    fn executor_steps_log_records_all_steps() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        exec.execute_all().unwrap();
        assert_eq!(exec.steps_log.len(), 3);
        assert_eq!(exec.steps_log[0].step_index, 0);
        assert_eq!(exec.steps_log[0].node_id, 2);
        assert_eq!(exec.steps_log[1].step_index, 1);
        assert_eq!(exec.steps_log[1].node_id, 3);
        assert_eq!(exec.steps_log[2].step_index, 2);
        assert_eq!(exec.steps_log[2].node_id, 1);
        for step in &exec.steps_log {
            assert!(step.success);
        }
    }

    #[test]
    fn executor_current_node_id_progression() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        assert_eq!(exec.current_node_id(), Some(2));
        exec.execute_next_step().unwrap();
        assert_eq!(exec.current_node_id(), Some(3));
        exec.execute_next_step().unwrap();
        assert_eq!(exec.current_node_id(), Some(1));
        exec.execute_next_step().unwrap();
        assert_eq!(exec.current_node_id(), None);
    }

    #[test]
    fn executor_availability_display_nonempty() {
        let c = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let avail = c.availability();
        let s = format!("{}", avail);
        assert!(s.contains("writable=true"));
        assert!(s.contains("readable=true"));
        assert!(s.contains("followers=2/2"));
    }

    #[test]
    fn executor_message_contains_leader_switch_info() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        exec.execute_next_step().unwrap();
        exec.execute_next_step().unwrap();
        let step3 = exec.execute_next_step().unwrap();
        assert!(step3.message.contains("leader switched"));
        assert!(step3.message.contains("1 → 2"));
        assert!(step3.message.contains("node 1 upgraded"));
    }

    #[test]
    fn executor_follower_message_no_leader_switch() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        let step1 = exec.execute_next_step().unwrap();
        assert!(!step1.message.contains("leader switched"));
        assert!(step1.message.contains("node 2 upgraded"));
    }

    #[test]
    fn end_to_end_patch_upgrade_business_no_outage() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 0, 1)).unwrap();
        let mut step_count = 0;
        while !exec.is_complete() {
            let step = exec.execute_next_step().unwrap();
            step_count += 1;
            assert!(step.availability_before.no_outage());
            assert!(step.availability_after.no_outage());
            let current_avail = exec.cluster.availability();
            assert!(current_avail.no_outage());
        }
        assert_eq!(step_count, 3);
        assert!(exec.cluster.all_versions_uniform());
    }

    #[test]
    fn end_to_end_minor_upgrade_business_no_outage() {
        let cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec = RollingUpgradeExecutor::new(cluster, NodeVersion::new(1, 2, 0)).unwrap();
        let steps = exec.execute_all().unwrap();
        for (i, step) in steps.iter().enumerate() {
            assert!(step.availability_after.no_outage(), "step {} outage", i + 1);
        }
        assert_eq!(exec.cluster.nodes[0].version, NodeVersion::new(1, 2, 0));
        assert_eq!(exec.cluster.nodes[1].version, NodeVersion::new(1, 2, 0));
        assert_eq!(exec.cluster.nodes[2].version, NodeVersion::new(1, 2, 0));
    }

    #[test]
    fn end_to_end_sequential_patch_upgrades() {
        let mut cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        for target in [1, 2, 3] {
            let mut exec =
                RollingUpgradeExecutor::new(cluster.clone(), NodeVersion::new(1, 0, target))
                    .unwrap();
            let steps = exec.execute_all().unwrap();
            assert_eq!(steps.len(), 3);
            for step in &steps {
                assert!(step.availability_after.no_outage());
            }
            cluster = exec.cluster;
            assert!(cluster.all_versions_uniform());
            assert_eq!(cluster.nodes[0].version, NodeVersion::new(1, 0, target));
        }
    }

    #[test]
    fn end_to_end_patch_then_minor_upgrade() {
        let mut cluster = Cluster::three_node_uniform(NodeVersion::new(1, 0, 0));
        let mut exec =
            RollingUpgradeExecutor::new(cluster.clone(), NodeVersion::new(1, 0, 5)).unwrap();
        exec.execute_all().unwrap();
        cluster = exec.cluster;
        assert!(cluster.all_versions_uniform());
        let mut exec2 =
            RollingUpgradeExecutor::new(cluster.clone(), NodeVersion::new(1, 2, 0)).unwrap();
        let steps = exec2.execute_all().unwrap();
        for step in &steps {
            assert!(step.availability_after.no_outage());
        }
        cluster = exec2.cluster;
        assert_eq!(cluster.nodes[0].version, NodeVersion::new(1, 2, 0));
    }

    #[test]
    fn error_messages_descriptive() {
        let inv = RollingError::InvalidVersion("bad".to_string()).to_string();
        assert!(inv.contains("invalid version"));
        assert!(inv.contains("bad"));
        let incompat = RollingError::IncompatibleVersion {
            from: NodeVersion::new(1, 0, 0),
            to: NodeVersion::new(2, 0, 0),
        }
        .to_string();
        assert!(incompat.contains("incompatible"));
        assert!(incompat.contains("1.0.0"));
        assert!(incompat.contains("2.0.0"));
        let not_up = RollingError::NotAnUpgrade {
            current: NodeVersion::new(1, 0, 0),
            target: NodeVersion::new(1, 0, 0),
        }
        .to_string();
        assert!(not_up.contains("not an upgrade"));
        let nf = RollingError::NoFollowerAvailable.to_string();
        assert!(nf.contains("no follower"));
        let nl = RollingError::NoLeaderAvailable.to_string();
        assert!(nl.contains("no leader"));
        let ac = RollingError::UpgradeAlreadyComplete.to_string();
        assert!(ac.contains("already complete"));
    }
}
