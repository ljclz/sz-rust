# ADR-0005: HLC Clock

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 时钟方案
- **相关代码**: `crates/szrsql-dist/src/hlc.rs` 或 `crates/szrsql-tx/src/clock.rs`
- **修复编号**: 无

## 背景

SzRSQL 分布式事务需要时间戳来：
1. 标识事务版本（MVCC snapshot）
2. 决定事务提交顺序（Percolator commit_ts）
3. 维持因果一致性（A happens-before B → ts(A) < ts(B)）

候选方案：

1. **物理时钟（Wall Clock）**：直接用系统时间，问题：NTP 同步误差（数十 ms），跨节点无法保证单调，可能违反因果。
2. **TrueTime（Google Spanner）**：GPS + 原子钟，误差 < 7ms，但需专用硬件，成本高。
3. **Vector Clock**：每个节点维护 (node_id, counter) 向量，问题：节点数 N 时元数据 O(N)，存储与带宽开销大。
4. **HLC（Hybrid Logical Clock）**：物理时钟 + 逻辑计数器，元数据 O(1)，因果一致性，无需专用硬件。

需求约束：
- 不依赖专用硬件（TrueTime 排除）
- 元数据紧凑（Vector Clock 在 N=100 节点不可接受）
- 因果一致性（物理时钟违反因果）
- 时钟漂移容忍（NTP 误差不影响正确性）

不选 HLC 的后果：
- 物理时钟导致因果倒序（A → B 但 ts(A) > ts(B)），MVCC 可见性判断错误
- TrueTime 成本高，无法社区部署
- Vector Clock 元数据膨胀，跨节点 RPC 开销大

## 决策

采用 HLC（Hybrid Logical Clock），结合物理时钟与逻辑计数器：

- **物理分量 `p`**：取自本地系统时钟（NTP 同步），毫秒精度
- **逻辑分量 `l`**：同一物理时刻内递增的逻辑计数器
- **HLC 比较**：先比 `p`，再比 `l`，保证偏序

HLC 算法（Kulkarni 2014）：

```rust
// 本地事件：递增 HLC
pub fn local_event(&mut self) -> Timestamp {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let pt_now = now.as_millis() as u64;
    if pt_now > self.pt {
        self.pt = pt_now;
        self.l = 0;
    } else {
        self.l += 1;
    }
    Timestamp { pt: self.pt, l: self.l }
}

// 接收远程事件：合并 HLC
pub fn receive(&mut self, remote: Timestamp) -> Timestamp {
    let pt_now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    if pt_now > self.pt && pt_now > remote.pt {
        self.pt = pt_now;
        self.l = 0;
    } else if remote.pt > self.pt {
        self.pt = remote.pt;
        self.l = remote.l + 1;
    } else if self.pt > remote.pt {
        self.l += 1;
    } else {
        // pt 相等
        self.l = max(self.l, remote.l) + 1;
    }
    Timestamp { pt: self.pt, l: self.l }
}
```

与 Percolator 集成：
- `start_ts` = 事务开始时调用 `local_event()` 获取
- `commit_ts` = 事务提交时调用 `local_event()` 获取，保证 `commit_ts > start_ts`
- 跨 shard 协调时通过 RPC 交换 HLC，调用 `receive()` 合并

## 后果

**正面**：
- 元数据 O(1)（仅 pt + l），跨节点 RPC 开销小
- 因果一致性：A → B 必有 hlc(A) < hlc(B)
- 无需专用硬件，社区可部署
- 时钟漂移容忍：物理时钟回退由逻辑计数器补偿

**负面**：
- 物理时钟漂移过大时，逻辑计数器频繁递增，HLC 偏离物理时间
- 不提供外部一致性（commit_ts 与真实时间可能差几秒），不像 TrueTime 那样有 bound
- 需要业务层处理 ts 比较的偏序关系（非全序）

## 注意事项

### 调用方约束
- 比较两个 HLC 必须 `pt` 优先，再 `l`，不可只比 `pt`
- 跨节点 RPC 必须在消息中携带 HLC，接收方调用 `receive()` 合并
- NTP 配置必须启用（漂移 > 500ms 时告警）
- 不可将 HLC 直接暴露给用户作为"时间"（它是逻辑时间，非物理时间）

### 迁移路径
- 单机房 HLC → 跨机房 HLC：需注意 NTP 同步质量
- 长期可替换为 TrueTime（如部署 GPS/原子钟）或混合方案（HLC + TrueTime bound）

### Bug 定位提示

**如果出现因果顺序异常（A → B 但 ts(A) > ts(B)）**：
1. **查 HLC 比较逻辑**：确认比较是 `pt` 优先再 `l`，不可只比 `pt` 或只比 `l`
2. **查 RPC 消息**：跨节点 RPC 是否携带 HLC，接收方是否调用 `receive()` 合并
3. **查本地事件**：本地事件是否调用 `local_event()` 递增 `l`，避免直接读系统时钟

**如果出现时钟倒流（同节点 ts 减小）**：
1. **查物理时钟漂移**：`SystemTime::now()` 是否回退（NTP 校时导致），HLC 应通过 `l += 1` 补偿
2. **查 HLC 状态持久化**：节点重启后 HLC 应从持久化恢复，否则 `l` 归零可能违反因果
3. **查并发更新**：HLC 更新是否加锁（`Mutex<HLC>`），并发更新可能导致 `l` 丢失递增

**如果出现 HLC 与物理时间偏差大（> 1s）**：
1. **查 NTP 配置**：`timedatectl status` 确认 NTP 同步正常
2. **查逻辑计数器 `l` 增长**：高频事务下 `l` 频繁递增，导致 HLC 远超物理时间
3. **查远程消息合并**：某节点物理时钟过快，导致其他节点通过 `receive()` 拉高 `pt`

**如果 MVCC 可见性判断错误**：
1. **查 snapshot_ts 来源**：snapshot 必须取自 HLC，不可取系统时钟
2. **查 commit_ts vs snapshot_ts**：可见性条件 `commit_ts <= snapshot_ts`
3. **可排除**：HLC 算法本身（经形式化验证），问题多在调用方误用

**如果跨 shard 事务 ts 不一致**：
1. **查 TSO 集中分配**：是否使用 Percolator TSO 统一分配 commit_ts
2. **查 shard 间 HLC 同步**：跨 shard RPC 是否携带 HLC 并合并
