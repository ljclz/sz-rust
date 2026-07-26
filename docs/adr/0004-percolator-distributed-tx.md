# ADR-0004: Percolator Distributed Tx

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 分布式事务
- **相关代码**: `crates/szrsql-dist/src/percolator.rs`（不存在则参考 `crates/szrsql-dist/src/raft.rs`）
- **修复编号**: 无

## 背景

SzRSQL 分布式版本需要支持跨 shard 事务，候选方案：

1. **2PC（Two-Phase Commit）**：协调者统一 preprepare + commit，缺点是协调者宕机导致阻塞，无锁清理机制。
2. **3PC**：增加 CanCommit 阶段减少阻塞，但消息开销大且仍存在不一致窗口。
3. **Saga**：长事务补偿模型，不保证隔离性，不适合 OLTP 强一致场景。
4. **Percolator**：Google 提出的基于 Bigtable 的事务模型，TiDB 已生产验证，单点协调者 + 主锁机制 + TSO 时间戳。

需求约束：
- 跨 shard ACID 保证
- 协调者故障后能自动清理锁（不死锁）
- 与现有 MVCC + HLC 时间戳方案集成
- 已有 TiKV/TiDB 大规模生产验证，工程可参考

不选 Percolator 的后果：
- 2PC 协调者宕机需人工干预或引入额外一致性协议
- 3PC 消息开销 3× 于 2PC，延迟高
- Saga 弱隔离，不适合金融场景

## 决策

采用 Percolator 模型，核心三要素：

1. **Timestamp Oracle (TSO)**：全局唯一时间戳分配器，单调递增。事务开始时获取 `start_ts`，提交时获取 `commit_ts`。
2. **两阶段提交**：
   - **Prewrite**：对每个 key 写入主锁（primary lock）或次锁（secondary lock），第一个 key 为主锁，其余为次锁指向主锁
   - **Commit**：先提交主锁（写 commit 记录 + commit_ts），再异步清理次锁
3. **单点协调者**：客户端充当协调者，primary lock 的状态决定事务成败

故障恢复机制：
- 任何节点发现其他事务的锁超时（默认 60s），可发起清理
- 清理次锁时需先查主锁状态：主锁已 commit → 次锁也 commit；主锁已回滚 → 次锁回滚

关键代码参考（`crates/szrsql-dist/src/percolator.rs`，若不存在应基于以下伪代码实现）：

```rust
pub struct PercolatorTxn {
    tso: TsoClient,
    shards: ShardRouter,
    start_ts: u64,
    primary_lock: Option<Key>,
    mutations: HashMap<Key, Value>,
}

// Prewrite 阶段
pub fn prewrite(&mut self) -> Result<(), TxError> {
    let primary = self.mutations.keys().next().unwrap().clone();
    self.primary_lock = Some(primary.clone());
    for (key, value) in &self.mutations {
        let is_primary = key == &primary;
        // 写入锁记录：(start_ts, primary_key, is_primary)
        self.shards.write_lock(*key, self.start_ts, primary.clone(), is_primary)?;
        // 写入数据（带 start_ts 版本）
        self.shards.write_data(*key, self.start_ts, value.clone())?;
    }
    Ok(())
}

// Commit 阶段
pub fn commit(&mut self) -> Result<(), TxError> {
    let commit_ts = self.tso.get_timestamp()?;
    // 先提交 primary lock
    let primary = self.primary_lock.as_ref().unwrap();
    self.shards.commit_key(primary, self.start_ts, commit_ts)?;
    // 异步清理 secondary locks（失败不影响事务结果）
    for key in self.mutations.keys() {
        if key != primary {
            let _ = self.shards.commit_key(key, self.start_ts, commit_ts);
        }
    }
    Ok(())
}
```

## 后果

**正面**：
- 协调者（客户端）无状态故障不阻塞系统（锁超时自动清理）
- 与 MVCC + HLC 天然集成（start_ts/commit_ts 即 HLC 时间戳）
- TiKV 大规模生产验证，方案成熟
- 单点 TSO 简化时间戳分配

**负面**：
- TSO 单点瓶颈（需主备 + 故障切换）
- 长事务持锁期间阻塞其他写（默认 60s 超时）
- 异步清理次锁期间，读请求需处理锁（read 慢路径）
- 跨 shard 延迟 = 2 RTT（prewrite + commit），高于单 shard

## 注意事项

### 调用方约束
- 事务开始必须先获取 `start_ts`，否则版本不可见
- 事务提交必须先 prewrite 全部 key，再 commit primary，最后异步 commit secondary
- 客户端宕机后，锁需等待 60s 超时由他人清理，期间阻塞同 key 写入
- TSO 故障期间系统不可写（必须 TSO 主备切换 < 10s）

### 迁移路径
- 当前：单 TSO（部署于 leader 节点）
- 中期：TSO 主备（基于 Raft 复制 timestamp 状态）
- 长期：分布式 TSO（如 Spanner TrueTime 替代，跨机房场景）

### Bug 定位提示

**如果事务卡住（hang 不返回）**：
1. **查锁清理**：grep `lock_ttl_expired` tracing span，确认超时锁是否被清理
2. **查 TSO 可用性**：`tso.get_timestamp()` 是否返回，TSO 节点是否在线
3. **查 primary lock 状态**：清理次锁时需先读 primary lock，若 primary lock 存在且未超时 → 等待；若 primary lock 不存在 → 数据不一致 bug

**如果出现超时（事务提交失败）**：
1. **查 TSO 可用性**：TSO 节点 CPU/网络/磁盘是否正常，是否发生主备切换
2. **查 prewrite 失败**：某个 shard 写锁是否成功（key 已有他人锁导致冲突）
3. **查网络延迟**：跨 shard RPC 是否超时（默认 RPC timeout 30s）

**如果出现 partial commit（部分 key commit 部分未 commit）**：
1. **查 primary lock 状态**：primary 已 commit 但 secondary 未 commit → 异步清理未完成（正常现象，读请求会触发 lock resolve）
2. **查 commit 顺序**：必须先 commit primary 再 commit secondary，顺序错误会导致事务状态不一致
3. **查 lock resolve 逻辑**：读请求遇到 secondary lock 时，应查 primary 状态并推进清理

**如果出现数据不一致（同一事务部分 key 可见部分不可见）**：
1. **查 commit_ts 一致性**：所有 key 必须用同一个 commit_ts
2. **查 MVCC 可见性判断**：`commit_ts <= reader_snapshot_ts` 才可见
3. **可排除**：Raft 复制层（Percolator 在 Raft 之上，复制不影响事务语义）

**如果 TSO 单点瓶颈（写入 QPS 上限）**：
1. **查 TSO batch size**：是否启用批量分配（每次批 100ms / 1000 个 ts）
2. **查 TSO 节点负载**：CPU/网络是否打满
3. **考虑迁移**：长期方案需分布式 TSO 或 HLC 替代
