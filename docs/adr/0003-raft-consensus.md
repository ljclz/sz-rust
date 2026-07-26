# ADR-0003: Raft Consensus

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 分布式共识
- **相关代码**: `crates/szrsql-dist/src/raft.rs (L1-L50)`
- **修复编号**: 无

## 背景

SzRSQL 分布式版本需要强一致性的复制层，候选方案有 Paxos、Multi-Paxos、Raft：

1. **Paxos**：理论完备但工程实现复杂，缺乏公认的标准多副本状态机实现。
2. **Multi-Paxos**：优化了 Paxos 的多提案场景，但 leader 选举与日志复制细节留给实现者，歧义多。
3. **Raft**：专为可理解性设计，leader-follower 模型清晰，term 编号单调递增，日志复制有明确语义。

不引入共识层的后果：
- 单节点故障即全系统不可用
- 跨节点数据复制只能依赖应用层异步同步，存在数据丢失风险
- 无法支持分布式事务的强一致性协调

需求约束：
- 必须支持 leader 选举（failover < 10s）
- 必须支持 log replication（顺序一致性）
- 必须支持 Multi-Raft（按 shard 分组，每组独立 Raft）
- 工程实现可维护，新成员能快速接手

## 决策

采用 Raft 算法，单 Raft 组 = 单 shard 复制组，Multi-Raft 通过多个 RaftNode 实例实现。

关键设计点：

- **leader-follower 模型**：每个 term 至多一个 leader，所有写请求经 leader 转发
- **term 单调递增**：选举时 term++，log entry 携带 term 用于识别过期 leader
- **log replication**：leader 接收写请求 → 追加本地 log → 复制到 follower → 多数确认后 commit
- **Multi-Raft**：每个 shard 一个 Raft 组，组间独立选举与日志复制

关键代码（`crates/szrsql-dist/src/raft.rs` L1-L50）：

```rust
// L1 RaftNode 核心
pub struct RaftNode {
    node_id: NodeId,
    current_term: u64,           // 单调递增
    voted_for: Option<NodeId>,
    log: Vec<LogEntry>,          // term + index + cmd
    commit_index: u64,
    last_applied: u64,
    state: RaftState,            // Follower | Candidate | Leader
    election_timeout: Duration,  // 随机化避免活锁
    heartbeat_interval: Duration,
    peers: Vec<NodeId>,
}

// L30 选举触发
pub fn start_election(&mut self) -> Result<(), RaftError> {
    self.current_term += 1;
    self.state = RaftState::Candidate;
    self.voted_for = Some(self.node_id);
    // 向 peers 发送 RequestVote(term, last_log_index, last_log_term)
    ...
}

// L45 日志复制
pub fn append_entries(&mut self, entries: Vec<LogEntry>, prev_log_index: u64, prev_log_term: u64) {
    // 校验 prev_log_index/prev_log_term 一致性
    // 不一致则拒绝，leader 退回 nextIndex 重试
    ...
}
```

## 后果

**正面**：
- 算法清晰，故障转移可预测（10s 内完成）
- 强一致性保证：committed log 在多数节点持久化
- Multi-Raft 横向扩展，单 shard 故障不影响其他 shard
- 丰富的开源参考（etcd、TiKV、LegoRaft）

**负面**：
- 写延迟 = 1 RTT（leader → follower → leader），跨机房延迟敏感
- leader 单点：leader 故障期间写不可用（直到选举完成）
- Multi-Raft 实现复杂，需共享心跳与 RPC 通道
- 不适合跨 WAN 强一致场景（应换用 Spanner-like Paxos+TrueTime）

## 注意事项

### 调用方约束
- 写请求必须发往 leader，follower 收到写需转发或拒绝（`NotLeader` 错误）
- 客户端需处理 `NotLeader` 并重试到新 leader
- `commit_index` 仅在 majority 确认后推进，不可单方面提交
- 选举超时必须 > 心跳间隔 × 2，否则误触发选举

### 迁移路径
- 单 Raft → Multi-Raft：通过 `RaftGroupManager` 管理多组
- 未来如需 WAN 复制：在 Raft 之上加 Async Raft Learner 跨机房异步同步

### Bug 定位提示

**如果出现脑裂（两个 leader 同时存在）**：
1. **查 term 编号**：两个 leader 的 term 必须不同，较低 term 的 leader 应被 `RequestVote` 高 term 覆盖；若同时存在说明网络分区未恢复或 RPC 失败
2. **查 election timeout**：是否过短导致 candidate 误判 leader 失联（应 ≥ heartbeat × 2）
3. **查投票约束**：每个 term 一个 node 只能投一票，查 `voted_for` 是否被错误重置

**如果出现日志不一致（follower 落后或冲突）**：
1. **查 prevLogIndex/prevLogTerm**：`append_entries` 必须校验这两个字段，不匹配则 leader 退回 nextIndex
2. **查 nextIndex 维护**：leader 是否在拒绝后递减 nextIndex 并重试
3. **查 log 截断**：follower 接收新 entry 前必须截断冲突的旧 entry（保留 index < prevLogIndex 的部分）

**如果出现 leader 频繁切换（选举抖动）**：
1. **查 election timeout 随机化**：所有 node 应使用 `rand(election_timeout, 2×election_timeout)` 避免同时竞选
2. **查心跳频率**：leader 心跳间隔是否过长导致 follower 误判
3. **查网络延迟**：RPC 延迟是否超过 election timeout（需调整 timeout 或优化网络）
4. **可排除**：业务层逻辑（leader 抖动是共识层问题）

**如果写入迟迟不 commit**：
1. **查 majority**：集群是否多数节点在线（3 节点集群至少 2 节点存活）
2. **查 follower 复制进度**：`match_index` 是否推进到 majority
3. **查 commit_index**：leader 是否在 majority 后更新 commit_index 并通知 follower
