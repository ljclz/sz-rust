# ADR-0006: Range Sharding

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 分片策略
- **相关代码**: `crates/szrsql-dist/src/shard.rs (L36-L131)`
- **修复编号**: 无

## 背景

SzRSQL 数据规模超过单机容量时，需将数据按 shard 分片存储。候选方案：

1. **Hash Sharding**：`shard_id = hash(key) % N`，优点：分布均匀；缺点：范围查询需广播所有 shard，无法支持 `WHERE k BETWEEN a AND b`。
2. **Consistent Hashing**：环状哈希空间，优点：节点增减时数据迁移少；缺点：仍是哈希，范围查询不友好。
3. **Range Sharding**：每个 shard 管理 `[start, end)` key 范围，优点：范围查询路由精准；缺点：热点 key 集中可能（如自增主键）。

需求约束：
- 大量范围查询（`WHERE id BETWEEN`、`ORDER BY`、`GROUP BY`）
- 动态分裂/合并 shard 以适应数据增长
- 跨 shard 事务（Percolator 已支持）
- 排序与索引扫描必须高效

不选 Range Sharding 的后果：
- Hash/Consistent Hash 下范围查询需广播 N 个 shard，N 大时延迟与带宽不可接受
- 排序需跨 shard merge，复杂度高
- 无法支持 prefix scan（如 `WHERE id LIKE 'abc%'`）

## 决策

采用 Range Sharding，每个 shard 管理 `[start_key, end_key)` 半开区间。

关键设计：

- **KeyRange**：`{ start: Vec<u8>, end: Vec<u8> }`，左闭右开
- **Shard**：`{ id: ShardId, range: KeyRange, replicas: Vec<NodeId>, leader: NodeId }`
- **ShardRouter**：维护 shard 元数据，根据 key 路由到对应 shard
- **动态分裂**：shard 数据量超过阈值（默认 96MB）时分裂为两个 shard
- **动态合并**：shard 数据量低于阈值（默认 8MB）时与相邻 shard 合并

关键代码（`crates/szrsql-dist/src/shard.rs` L36-L131）：

```rust
// L36 KeyRange 定义
#[derive(Clone, Debug)]
pub struct KeyRange {
    pub start: Vec<u8>,   // 包含
    pub end: Vec<u8>,     // 不包含
}

// L50 Shard 定义
pub struct Shard {
    pub id: ShardId,
    pub range: KeyRange,
    pub replicas: Vec<NodeId>,
    pub leader: NodeId,
    pub size_bytes: u64,
}

// L80 ShardRouter 路由
pub struct ShardRouter {
    shards: BTreeMap<Vec<u8>, Shard>,  // 按 start_key 排序
}

impl ShardRouter {
    // L100 路由：根据 key 找到所属 shard
    pub fn route(&self, key: &[u8]) -> Result<&Shard, RouteError> {
        let shard = self.shards.range(..=key.to_vec()).next_back()
            .ok_or(RouteError::NoShardFound)?;
        if key >= shard.1.range.end.as_slice() {
            return Err(RouteError::KeyOutOfRange);
        }
        Ok(shard.1)
    }

    // L120 范围路由：找出覆盖 [start, end) 的所有 shard
    pub fn route_range(&self, start: &[u8], end: &[u8]) -> Vec<&Shard> {
        self.shards.range(start.to_vec()..end.to_vec())
            .map(|(_, s)| s)
            .collect()
    }

    // L130 分裂 shard
    pub fn split_shard(&mut self, shard_id: ShardId, split_key: Vec<u8>) -> Result<(), SplitError> {
        // 1. 校验 split_key 在 shard 范围内
        // 2. 创建新 shard，元数据提交到 Raft
        // 3. 等待 majority 确认
        ...
    }
}
```

## 后果

**正面**：
- 范围查询路由精准，无需广播（`route_range` 返回相关 shard 子集）
- 排序与索引扫描可在 shard 内完成，跨 shard 仅需 merge
- prefix scan 自然支持（前缀相同的 key 在同一 shard）
- 动态分裂/合并适应数据增长

**负面**：
- 热点 key 问题：自增主键 / 时间戳作为 key 时，写入集中到最后一个 shard
- 分裂/合并期间短暂影响可用性（需迁移数据 + 切换路由）
- 元数据需全局一致（依赖 Raft 复制 shard 元信息）
- 跨 shard 事务需 Percolator 协调（已支持）

## 注意事项

### 调用方约束
- 所有读写必须经 `ShardRouter.route()` 定位 shard，不可直接访问
- 范围查询必须用 `route_range()`，不可只路由 start key
- 客户端需缓存 shard 路由表，遇到 `NotLeader` / `ShardSplit` 错误时刷新缓存
- 写入 key 应避免单调递增（如自增 id），可加随机前缀打散热点

### 迁移路径
- 单 shard → 多 shard：通过 `split_shard` 分裂
- 热点 shard 缓解：使用 pre-split（建表时预分配多个 shard）或 hashed key 前缀
- 未来可引入动态热点检测 + 自动分裂

### Bug 定位提示

**如果出现路由错误（数据写到错误 shard 或读不到数据）**：
1. **查 `ShardRouter.route()` 逻辑**：`BTreeMap.range(..=key).next_back()` 是否正确，`end` 边界是否包含
2. **查 shard 元数据**：`start`/`end` 是否重叠或空洞，分裂/合并后元数据是否一致
3. **查客户端缓存**：客户端路由表是否过期，遇到 `ShardSplit` 错误是否刷新

**如果出现热点 shard（CPU/IO 不均衡）**：
1. **查 shard size 分布**：`SELECT shard_id, size_bytes FROM shards` 是否严重不均
2. **查 key 分布**：业务 key 是否单调递增（如自增 id、时间戳）
3. **缓解措施**：手动 `split_shard` 或加随机前缀重写 key

**如果 split 失败（shard 长期超阈值未分裂）**：
1. **查 metadata quorum**：split 元数据是否提交到 Raft majority，网络分区下可能阻塞
2. **查 leader 状态**：shard leader 是否正常，leader 切换期间无法 split
3. **查 split_key 选择**：split_key 是否在 shard 范围内，是否选中了空 key（导致分裂不均）

**如果范围查询结果不完整（少数据）**：
1. **查 `route_range()` 实现**：`BTreeMap.range(start..end)` 是否包含边界 shard
2. **查 shard 边界**：相邻 shard 的 `start`/`end` 是否连续无空洞
3. **可排除**：业务逻辑（范围查询错误通常是路由问题）

**如果出现跨 shard 事务延迟高**：
1. **查涉及 shard 数量**：事务涉及 shard 多则 Percolator 协调开销大
2. **查 shard 分布**：shard 是否跨机房（应优先同机房副本）
3. **优化**：业务设计减少跨 shard 事务（如将相关 key 放同一 shard）
