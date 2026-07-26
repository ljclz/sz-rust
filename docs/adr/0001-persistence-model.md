# ADR-0001: 持久性模型：当前状态与 log-then-commit 迁移路径

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 存储引擎
- **相关代码**: `crates/szrsql-tx/src/wal.rs (L312-L371)`, `crates/szrsql-tx/src/mvcc.rs (L560-L612)`, `crates/szrsql-sql/src/executor.rs (L3436-L3517)`
- **修复编号**: Critical C-1（持久性模型风险）

## 背景

SzRSQL 当前的持久性模型存在严重架构缺陷，必须明确记录现状并规划迁移路径。

### 现状分析（2026-07-24 实测）

通过对代码的实测审查，发现以下事实：

1. **SQL 执行路径无 WAL 集成**：
   - `szrsql-sql` crate 的 `Cargo.toml` 不依赖 `szrsql-tx`
   - `executor.rs` 的 `execute()` 方法不调用 `wal.append()` 也不调用 `wal.flush()`
   - SQL 执行仅依赖内存存储 + snapshot/restore 实现事务隔离（见 `executor.rs` L29 注释）
   - **后果**：SQL 层的 DML 操作完全无持久性保证，进程崩溃即丢失全部数据

2. **MVCC `commit` API 设计正确但未集成**：
   - `mvcc.rs` L567 的 `commit(txn_id: u32, commit_lsn: u64)` 接受 `commit_lsn` 参数
   - 该参数的设计意图是：调用方先 `wal.append()` 获取 LSN，再传入 commit
   - 但实际调用场景（`mvcc_fuzz.rs`、`isolation_fuzz.rs`、`lock_fuzz.rs`）传入的 `commit_lsn` 是 `0`、`100`、`rng.next_u64()` 等任意值
   - **后果**：MVCC 层有正确的 API 设计但从未被正确使用

3. **WAL `append` 不 fsync**：
   - `wal.rs` L318 的 `append()` 仅调用 `file.write_all()`，写入 OS buffer
   - 注释明确写道："数据写入操作系统缓冲区（不保证持久化，需调用 `flush()`）"
   - `flush()` 方法（L365）才执行真正的 `fsync`
   - **后果**：即使集成 WAL，若不调用 `flush()`，仍然无法保证持久性

4. **GroupCommit 存在但未集成到 SQL 路径**：
   - `wal.rs` L536 的 `GroupCommit` 包装器支持批量 fsync（每 `batch_threshold` 条触发一次）
   - 仅在 `jepsen_*.rs`、`crash_recovery_fuzz.rs` 测试中使用

### 不解决的后果

- **数据丢失**：任何进程崩溃（kill -9、OOM、断电）都会丢失全部已提交事务
- **无法上生产**：Durability 不满足 ACID，不具备数据库最基本的持久性保证
- **分布式层假象**：Raft 复制层虽能跨节点复制，但源节点本身无 WAL，复制的是内存状态，节点重启后状态丢失

## 决策

### 短期（v0.3.0）：明确现状，标记为"内存数据库"

接受当前状态为"内存数据库"（in-memory database），明确不支持崩溃恢复。所有文档、配置、用户界面必须明确标注此限制。

```rust
// szrsql-sql/src/executor.rs L29 现有注释保持
//! - **事务用 snapshot/restore**：Phase 3.5 简化事务模型，完整 ACID 留待 szrsql-tx 子系统
```

### 中期（v0.4.0）：实现 log-then-commit 集成

将 SQL 执行路径与 WAL 集成，采用严格的 log-then-commit 模型：

```rust
// 目标集成模式（伪代码）
pub fn execute_write(&self, plan: &LogicalPlan) -> Result<Vec<Row>, ExecutionError> {
    let txn_id = self.begin_txn()?;
    let wal_record = WalRecord::new(txn_id, OpType::Write, plan.encode());
    
    // 1. 先写 WAL（log）
    let commit_lsn = self.wal.append(wal_record)?;
    self.wal.flush()?;  // 强制 fsync，保证持久化
    
    // 2. 再提交事务（commit）
    self.mvcc.commit(txn_id, commit_lsn)?;
    
    // 3. 应用变更到存储
    self.apply_changes(plan)?;
    
    Ok(vec![])
}
```

### 长期（v0.5.0+）：Group Commit 优化

引入 `GroupCommit` 包装器，将 fsync 开销摊销到多条记录：
- `batch_threshold = 128`（默认）
- 同步等待时间 ≤ 10ms
- 崩溃时最多丢失 `batch_threshold` 条未 fsync 的记录（可接受）

## 后果

**正面**：
- 明确现状，避免误用为持久化数据库
- 提供清晰的迁移路径，v0.4.0 可实现完整 ACID
- MVCC API 设计已经正确，无需破坏性变更

**负面**：
- v0.3.0 之前无法用于生产环境（仅适用于缓存/会话存储等可丢数据场景）
- log-then-commit 引入 fsync 延迟（单条 ~5ms on Windows, ~1ms on Linux SSD）
- Group Commit 牺牲少量持久性换取性能（最多丢 128 条）

## 注意事项

### 调用方约束
- v0.3.x 用户必须明确知道数据不持久化，启动脚本需显示警告
- v0.4.0+ 启用 WAL 后，必须配置 WAL 目录和 fsync 策略
- Group Commit 的 `batch_threshold` 必须根据业务 RPO 调整

### 迁移路径
1. **v0.4.0-alpha**：szrsql-sql 添加 szrsql-tx 依赖，execute_write 集成 WAL
2. **v0.4.0-beta**：添加 `wal_sync_strategy` 配置（`always` | `group_commit` | `none`）
3. **v0.4.0-rc**：崩溃恢复测试，验证 log-then-commit 正确性
4. **v0.4.0**：生产可用，默认 `group_commit` 策略

### Bug 定位提示

**如果 commit 后宕机导致数据丢失**：
1. **检查 SQL 层是否集成 WAL**：grep `szrsql-sql/Cargo.toml` 是否有 `szrsql-tx` 依赖；若无 → 已知限制（本 ADR 记录）
2. **检查 WAL append 是否在 commit 前**：查 `executor.rs` 的 `execute_write` 调用顺序，确认 `wal.append()` + `wal.flush()` 在 `mvcc.commit()` 之前
3. **检查 fsync 是否成功**：查 tracing span `wal.fsync` 的返回值，确认 `WalError::IoError` 未发生
4. **检查 Group Commit 阈值**：若使用 GroupCommit，查 `batch_threshold` 和 `pending_count`，确认未 fsync 的记录数是否在 RPO 范围内
5. **可排除**：Raft 复制层（单节点宕机不涉及共识）；MVCC 层（API 设计正确，问题在调用方）

**如果崩溃恢复后数据不一致**：
1. **检查 WAL replay 顺序**：replay 必须按 LSN 严格递增
2. **检查 checkpoint**：checkpoint 之前的 WAL 可安全截断，之后的必须 replay
3. **检查事务状态**：未 commit 的事务必须 rollback（undo log 或 WAL compensation record）
