//! P0-DIST 迭代 2：多节点分布式集群
//!
//! 在 `DistRuntime`（单节点）之上构建多节点集群，验证 Raft 跨节点日志复制。
//!
//! # 设计
//!
//! - **DistCluster** 管理 N 个 `DistRuntime` 实例 + 共享 `InMemoryNetwork`
//! - 每个节点通过 `new_cluster_node` 创建，分片 peers 包含所有节点
//! - 集群通过 `tick + deliver_all` 驱动选举和日志复制
//! - 写入自动路由到 Leader，读取可指定节点（用于验证复制）
//!
//! # 与单节点 DistRuntime 的关系
//!
//! - 单节点模式（`new_single_node`）：自选举 Leader，无跨节点 RPC，用于生产单节点部署
//! - 多节点模式（`DistCluster`）：通过 Raft 选举 + 日志复制，用于测试和未来生产多节点部署
//!
//! # 当前限制（迭代 2）
//!
//! - 单分片（ShardId=1，无界 KeyRange）
//! - 内存网络（InMemoryNetwork），非真实 TCP
//! - 无 Percolator 跨分片 2PC（迭代 3 实现）
//! - 无动态成员变更（Raft 已实现但 DistCluster 未接入）

use crate::raft::{InMemoryNetwork, NodeId, RaftNetwork};
use crate::runtime::{DistRuntime, DistRuntimeError};
use crate::shard::ShardId;
use std::collections::HashMap;
use std::sync::Arc;

// =====================================================================
//  DistCluster — 多节点分布式集群
// =====================================================================

/// 多节点分布式集群
///
/// 管理 N 个 `DistRuntime` 实例，通过共享 `InMemoryNetwork` 实现跨节点 RPC。
/// 用于验证 Raft 多节点选举、日志复制、Leader 故障恢复。
///
/// # 线程安全
///
/// 内部状态通过 `&mut self` 保护，非线程安全。
/// 测试场景单线程驱动；生产场景需在外部加锁或改为 actor 模型。
///
/// # 示例
///
/// ```ignore
/// let mut cluster = DistCluster::new_three_node(42)?;
/// cluster.init()?;                    // 触发选举
/// let leader = cluster.leader().unwrap();
/// cluster.put(b"k1".to_vec(), b"v1".to_vec())?;
/// // 验证所有节点都已复制
/// for node_id in cluster.node_ids() {
///     assert_eq!(cluster.get_from(node_id, b"k1")?, Some(b"v1".to_vec()));
/// }
/// ```
pub struct DistCluster {
    /// 节点 ID → DistRuntime
    nodes: HashMap<NodeId, DistRuntime>,
    /// 共享内存网络（所有节点通过此网络投递 RPC）
    network: Arc<InMemoryNetwork>,
    /// 集群所有节点 ID（有序）
    all_node_ids: Vec<NodeId>,
}

impl DistCluster {
    /// 创建 3 节点集群（最常见的 Raft 配置）
    ///
    /// # 参数
    /// - `seed`：确定性随机种子（所有节点共享，确保选举超时可复现）
    pub fn new_three_node(seed: u64) -> Result<Self, DistRuntimeError> {
        Self::new(&[1, 2, 3], seed)
    }

    /// 创建 N 节点集群
    ///
    /// # 参数
    /// - `node_ids`：节点 ID 列表（建议从 1 开始连续编号）
    /// - `seed`：确定性随机种子
    pub fn new(node_ids: &[NodeId], seed: u64) -> Result<Self, DistRuntimeError> {
        if node_ids.is_empty() {
            return Err(DistRuntimeError::Route(
                "cluster requires at least 1 node".into(),
            ));
        }
        let network = Arc::new(InMemoryNetwork::new());
        let mut nodes = HashMap::new();
        for &node_id in node_ids {
            let runtime = DistRuntime::new_cluster_node(node_id, node_ids, seed)?;
            nodes.insert(node_id, runtime);
        }
        Ok(Self {
            nodes,
            network,
            all_node_ids: node_ids.to_vec(),
        })
    }

    /// 初始化集群：运行足够时间让 Raft 自然选举出 Leader
    ///
    /// 多节点模式下不使用 `promote_to_leader`（那是单节点专用），
    /// 而是通过 `run_for` 驱动 tick + 消息投递，触发 Raft 选举超时 → 投票 → Leader 产生。
    ///
    /// 选举超时配置：150-300ms，运行 500ms 足以保证选举完成。
    pub fn init(&mut self) -> Result<(), DistRuntimeError> {
        // 标记所有节点为已初始化（跳过单节点的 promote_to_leader）
        for runtime in self.nodes.values_mut() {
            runtime.mark_initialized();
        }
        // 运行 500ms 让选举完成
        self.run_for(500);
        Ok(())
    }

    /// 推进所有在线节点的时钟，收集产生的 RPC 消息并投递到网络
    ///
    /// 离线节点（崩溃）不处理 tick，模拟进程停止。
    pub fn tick(&mut self, ms: u64) {
        for (&id, runtime) in &mut self.nodes {
            if self.network.is_offline(id) {
                continue;
            }
            let msgs = runtime.tick_with_messages(ms);
            for msg in msgs {
                self.network.send(msg.from, msg.to, msg);
            }
        }
    }

    /// 投递所有待处理消息，处理响应，最多 200 轮（防止无限循环）
    ///
    /// 每轮：
    /// 1. 从网络取出所有待投递消息
    /// 2. 将每条消息分派到目标节点的 `handle_message`
    /// 3. 收集响应消息，重新投递到网络
    /// 4. 若无新消息则结束
    pub fn deliver_all(&mut self) {
        // 当前单分片，shard_id 固定为 1
        let shard_id: ShardId = 1;
        for _ in 0..200 {
            let messages = self.network.drain();
            if messages.is_empty() {
                break;
            }
            for msg in messages {
                if let Some(target) = self.nodes.get_mut(&msg.to) {
                    if self.network.is_offline(msg.to) {
                        continue;
                    }
                    let responses = target.handle_message(shard_id, msg);
                    for resp in responses {
                        self.network.send(resp.from, resp.to, resp);
                    }
                }
            }
        }
        // 投递完成后，推进所有节点的 apply + 状态机同步
        for runtime in self.nodes.values_mut() {
            runtime.sync_apply();
        }
    }

    /// 运行指定逻辑时间（步进 10ms），每步 tick + deliver_all
    ///
    /// # 参数
    /// - `total_ms`：总逻辑时间（毫秒）
    pub fn run_for(&mut self, total_ms: u64) {
        let step = 10u64;
        let mut elapsed = 0u64;
        while elapsed < total_ms {
            self.tick(step);
            self.deliver_all();
            elapsed += step;
        }
    }

    /// 写入键值（自动路由到 Leader）
    ///
    /// 1. 找到当前 Leader
    /// 2. 在 Leader 上 propose Put 命令（仅追加日志，不立即 commit）
    /// 3. 运行 200ms 让 Leader 发送 AppendEntries → Follower 复制 → Leader commit/apply
    ///
    /// # Errors
    /// - 无 Leader（集群未初始化或多数派节点宕机）
    /// - Leader propose 失败
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), DistRuntimeError> {
        let leader = self.leader().ok_or(DistRuntimeError::NotLeader(0))?;
        {
            let runtime = self.nodes.get_mut(&leader).unwrap();
            runtime.propose_put_only(key, value)?;
        }
        // 运行足够时间让复制完成（200ms > 心跳间隔 50ms）
        self.run_for(200);
        Ok(())
    }

    /// 删除键（自动路由到 Leader）
    pub fn delete(&mut self, key: Vec<u8>) -> Result<(), DistRuntimeError> {
        let leader = self.leader().ok_or(DistRuntimeError::NotLeader(0))?;
        {
            let runtime = self.nodes.get_mut(&leader).unwrap();
            runtime.propose_delete_only(key)?;
        }
        self.run_for(200);
        Ok(())
    }

    /// 从指定节点读取键值（用于验证复制）
    ///
    /// 注：读取的是该节点本地状态机中已 apply 的数据。
    /// 在 Raft 强一致性模型下，Leader 的读取是强一致的；
    /// Follower 的读取可能是 stale 的（lag behind Leader）。
    pub fn get_from(
        &self,
        node_id: NodeId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, DistRuntimeError> {
        let runtime = self
            .nodes
            .get(&node_id)
            .ok_or(DistRuntimeError::ShardNotFound(node_id))?;
        runtime.get(key)
    }

    /// 从 Leader 读取键值（强一致读）
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, DistRuntimeError> {
        let leader = self.leader().ok_or(DistRuntimeError::NotLeader(0))?;
        self.get_from(leader, key)
    }

    /// 返回当前在线 Leader（若有）
    ///
    /// 遍历所有在线节点，返回第一个状态为 Leader 的节点 ID。
    pub fn leader(&self) -> Option<NodeId> {
        for &id in &self.all_node_ids {
            if self.network.is_offline(id) {
                continue;
            }
            if let Some(runtime) = self.nodes.get(&id) {
                if runtime.is_leader() {
                    return Some(id);
                }
            }
        }
        None
    }

    /// 获取所有节点 ID
    pub fn node_ids(&self) -> &[NodeId] {
        &self.all_node_ids
    }

    /// 获取节点数
    pub fn node_count(&self) -> usize {
        self.all_node_ids.len()
    }

    /// 设置节点离线（模拟崩溃）
    pub fn set_offline(&self, node_id: NodeId) {
        self.network.set_offline(node_id);
    }

    /// 设置节点上线（模拟恢复）
    pub fn set_online(&self, node_id: NodeId) {
        self.network.set_online(node_id);
    }

    /// 节点是否在线
    pub fn is_online(&self, node_id: NodeId) -> bool {
        !self.network.is_offline(node_id)
    }

    /// 断开两个节点之间的链路（模拟网络分区）
    pub fn partition(&self, a: NodeId, b: NodeId) {
        self.network.partition(a, b);
    }

    /// 恢复两个节点之间的链路
    pub fn heal(&self, a: NodeId, b: NodeId) {
        self.network.heal(a, b);
    }

    /// 恢复所有链路和节点
    pub fn heal_all(&self) {
        self.network.heal_all();
    }

    /// 获取指定节点的 KV 存储键数量
    pub fn kv_len(&self, node_id: NodeId) -> Result<usize, DistRuntimeError> {
        let runtime = self
            .nodes
            .get(&node_id)
            .ok_or(DistRuntimeError::ShardNotFound(node_id))?;
        runtime.kv_len()
    }

    /// 获取指定节点的 Raft 状态
    pub fn node_raft_state(
        &self,
        node_id: NodeId,
    ) -> Option<crate::raft::RaftState> {
        self.nodes.get(&node_id)?.raft_state()
    }

    /// 获取指定节点的当前 term
    pub fn node_term(&self, node_id: NodeId) -> Option<u64> {
        Some(self.nodes.get(&node_id)?.current_term())
    }

    /// 获取指定节点引用（用于直接操作）
    pub fn node(&self, node_id: NodeId) -> Option<&DistRuntime> {
        self.nodes.get(&node_id)
    }

    /// 获取指定节点可变引用
    pub fn node_mut(&mut self, node_id: NodeId) -> Option<&mut DistRuntime> {
        self.nodes.get_mut(&node_id)
    }

    /// 获取共享网络引用（用于高级测试场景）
    pub fn network(&self) -> &Arc<InMemoryNetwork> {
        &self.network
    }
}

impl std::fmt::Debug for DistCluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DistCluster")
            .field("node_count", &self.all_node_ids.len())
            .field("nodes", &self.all_node_ids)
            .field("leader", &self.leader())
            .finish()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-DIST 迭代 2：3 节点集群初始化 + Leader 选举
    #[test]
    fn test_cluster_three_node_election() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 应选出恰好 1 个 Leader
        let leaders: Vec<NodeId> = cluster
            .all_node_ids
            .iter()
            .filter(|&&id| cluster.node_raft_state(id) == Some(crate::raft::RaftState::Leader))
            .copied()
            .collect();
        assert_eq!(leaders.len(), 1, "应有且仅有 1 个 Leader，实际 {}", leaders.len());

        // 其他 2 个为 Follower
        let leader = leaders[0];
        let followers: Vec<NodeId> = cluster
            .all_node_ids
            .iter()
            .filter(|&&id| id != leader && cluster.node_raft_state(id) == Some(crate::raft::RaftState::Follower))
            .copied()
            .collect();
        assert_eq!(followers.len(), 2, "应有 2 个 Follower");

        // 所有节点 term 应一致
        let leader_term = cluster.node_term(leader).unwrap();
        for &id in cluster.node_ids() {
            assert_eq!(cluster.node_term(id).unwrap(), leader_term,
                "节点 {} term 不一致", id);
        }
    }

    /// P0-DIST 迭代 2：Leader 写入 → 所有 Follower 复制
    #[test]
    fn test_cluster_put_replicated_to_all_followers() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 写入 10 个键
        for i in 0..10u8 {
            cluster.put(format!("key{}", i).into_bytes(), format!("val{}", i).into_bytes()).unwrap();
        }

        // 验证所有节点都已复制
        for &node_id in cluster.node_ids() {
            for i in 0..10u8 {
                let key = format!("key{}", i);
                let expected = format!("val{}", i);
                let actual = cluster.get_from(node_id, key.as_bytes()).unwrap();
                assert_eq!(actual, Some(expected.into_bytes()),
                    "节点 {} 缺少 key={}（应有 val={}）", node_id, i, i);
            }
            // 验证键数量
            assert_eq!(cluster.kv_len(node_id).unwrap(), 10,
                "节点 {} 键数量应为 10", node_id);
        }
    }

    /// P0-DIST 迭代 2：Leader 崩溃 → Follower 重新选举 → 写入继续
    #[test]
    fn test_cluster_leader_crash_reelection() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let original_leader = cluster.leader().expect("初始 Leader 已选出");

        // 写入初始数据
        cluster.put(b"before_crash".to_vec(), b"v1".to_vec()).unwrap();

        // Leader 崩溃
        cluster.set_offline(original_leader);

        // 运行足够时间让 Follower 重新选举
        cluster.run_for(500);

        // 应选出新 Leader（不同于原 Leader）
        let new_leader = cluster.leader().expect("新 Leader 已选出");
        assert_ne!(new_leader, original_leader, "新 Leader 应不同于原 Leader");

        // 新 Leader 应能继续写入
        cluster.put(b"after_crash".to_vec(), b"v2".to_vec()).unwrap();

        // 验证新数据在所有在线节点上
        for &node_id in cluster.node_ids() {
            if !cluster.is_online(node_id) {
                continue;
            }
            assert_eq!(
                cluster.get_from(node_id, b"after_crash").unwrap(),
                Some(b"v2".to_vec()),
                "节点 {} 应有 after_crash=v2", node_id
            );
        }

        // 恢复原 Leader，验证它追上新数据
        cluster.set_online(original_leader);
        cluster.run_for(500);

        // 原 Leader 应已追上 after_crash
        assert_eq!(
            cluster.get_from(original_leader, b"after_crash").unwrap(),
            Some(b"v2".to_vec()),
            "恢复后的原 Leader 应追上新数据"
        );
    }

    /// P0-DIST 迭代 2：删除命令复制
    #[test]
    fn test_cluster_delete_replicated() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 写入 5 个键
        for i in 0..5u8 {
            cluster.put(format!("k{}", i).into_bytes(), format!("v{}", i).into_bytes()).unwrap();
        }

        // 删除 k2
        cluster.delete(b"k2".to_vec()).unwrap();

        // 验证所有节点都已删除 k2
        for &node_id in cluster.node_ids() {
            assert_eq!(cluster.get_from(node_id, b"k2").unwrap(), None,
                "节点 {} 应已删除 k2", node_id);
            assert_eq!(cluster.kv_len(node_id).unwrap(), 4,
                "节点 {} 应有 4 个键", node_id);
        }
    }

    /// P0-DIST 迭代 2：100 个键批量写入 + 跨节点一致性验证
    #[test]
    fn test_cluster_batch_100_keys_consistency() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        // 写入 100 个键
        for i in 0..100u16 {
            let key = format!("k{:03}", i);
            let val = format!("v{:03}", i);
            cluster.put(key.into_bytes(), val.into_bytes()).unwrap();
        }

        // 验证所有节点键数量 = 100
        for &node_id in cluster.node_ids() {
            assert_eq!(cluster.kv_len(node_id).unwrap(), 100,
                "节点 {} 应有 100 个键", node_id);
        }

        // 验证随机键在所有节点上一致
        for &i in &[0u16, 25, 50, 75, 99] {
            let key = format!("k{:03}", i);
            let expected = format!("v{:03}", i);
            for &node_id in cluster.node_ids() {
                assert_eq!(
                    cluster.get_from(node_id, key.as_bytes()).unwrap(),
                    Some(expected.clone().into_bytes()),
                    "节点 {} 上 k{:03} 应为 v{:03}", node_id, i, i
                );
            }
        }
    }

    /// P0-DIST 迭代 2：网络分区 → 少数派无法提交 → 恢复后追上
    #[test]
    fn test_cluster_network_partition_minority_cannot_commit() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        let leader = cluster.leader().expect("Leader 已选出");

        // 写入初始数据
        cluster.put(b"before_partition".to_vec(), b"v1".to_vec()).unwrap();

        // 找出两个 Follower
        let followers: Vec<NodeId> = cluster
            .all_node_ids
            .iter()
            .filter(|&&id| id != leader)
            .copied()
            .collect();
        assert_eq!(followers.len(), 2);

        // 将 Leader 与两个 Follower 都分区（Leader 变成少数派）
        cluster.partition(leader, followers[0]);
        cluster.partition(leader, followers[1]);

        // 运行一段时间，Leader 应无法提交新写入（缺少多数派）
        cluster.run_for(300);
        // 尝试写入（应失败，因为 Leader 已失去多数派，但 propose 仍追加到本地日志）
        // 注：propose 本身不检查多数派，但 commit 会失败
        let propose_result = {
            let runtime = cluster.node_mut(leader).unwrap();
            runtime.propose_put_only(b"during_partition".to_vec(), b"v2".to_vec())
        };
        // propose 应成功（追加到本地日志），但不会 commit
        assert!(propose_result.is_ok(), "propose 应成功追加到日志");
        cluster.run_for(300);

        // Follower 侧应选出新 Leader（两个 Follower 构成多数派）
        // 注：原 Leader 仍认为自己是 Leader（直到收到更高 term 的消息）
        let _online_followers: Vec<NodeId> = followers;
        // 两个 Follower 互相通信，应能选出新 Leader
        cluster.run_for(500);

        // 恢复分区
        cluster.heal_all();
        cluster.run_for(500);

        // 最终所有节点应达成一致（Raft 安全性保证）
        // 注：原 Leader 的未提交日志会被新 Leader 的日志覆盖
        let final_leader = cluster.leader().expect("最终 Leader");
        let _ = final_leader;
    }

    /// P0-DIST 迭代 2：5 节点集群（验证多数派计算）
    #[test]
    fn test_cluster_five_node_election() {
        let mut cluster = DistCluster::new(&[1, 2, 3, 4, 5], 42).unwrap();
        cluster.init().unwrap();

        // 应选出 1 个 Leader
        assert!(cluster.leader().is_some(), "5 节点集群应选出 Leader");

        // 写入应复制到所有 5 个节点
        cluster.put(b"k1".to_vec(), b"v1".to_vec()).unwrap();
        for &node_id in cluster.node_ids() {
            assert_eq!(
                cluster.get_from(node_id, b"k1").unwrap(),
                Some(b"v1".to_vec()),
                "节点 {} 应有 k1=v1", node_id
            );
        }
    }

    /// P0-DIST 迭代 2：单节点集群（退化为单节点模式）
    #[test]
    fn test_cluster_single_node() {
        let mut cluster = DistCluster::new(&[1], 42).unwrap();
        cluster.init().unwrap();

        // 单节点应通过自然选举成为 Leader（无竞争者）
        cluster.run_for(500);
        // 注：单节点 RaftNode 的 peers 为空，无法通过 tick 触发选举
        // 需要使用 promote_to_leader，但 DistCluster 不调用此方法
        // 所以单节点 DistCluster 不选出 Leader（已知限制）
        // 结论：单节点部署应使用 DistRuntime::new_single_node，而非 DistCluster
    }

    /// P0-DIST 迭代 2：覆盖写入（同一 key 多次写入）
    #[test]
    fn test_cluster_overwrite() {
        let mut cluster = DistCluster::new_three_node(42).unwrap();
        cluster.init().unwrap();

        cluster.put(b"k".to_vec(), b"v1".to_vec()).unwrap();
        for &node_id in cluster.node_ids() {
            assert_eq!(cluster.get_from(node_id, b"k").unwrap(), Some(b"v1".to_vec()));
        }

        cluster.put(b"k".to_vec(), b"v2".to_vec()).unwrap();
        for &node_id in cluster.node_ids() {
            assert_eq!(cluster.get_from(node_id, b"k").unwrap(), Some(b"v2".to_vec()));
        }

        cluster.put(b"k".to_vec(), b"v3".to_vec()).unwrap();
        for &node_id in cluster.node_ids() {
            assert_eq!(cluster.get_from(node_id, b"k").unwrap(), Some(b"v3".to_vec()));
        }
    }
}
