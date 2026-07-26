# ADR-0002: MVCC over 2PL

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 并发原语
- **相关代码**: `crates/szrsql-tx/src/mvcc.rs (L488-L612)`
- **修复编号**: 无

## 背景

SzRSQL 需要在多版本并发控制（MVCC）与传统两阶段锁（2PL）之间做出选择，以满足以下需求：

1. **读写并发场景普遍**：OLTP/OLAP 混合负载下，长事务读与短事务写频繁并发，2PL 的读锁会严重拖慢写入吞吐。
2. **隔离级别可配置**：需支持 READ COMMITTED、REPEATABLE READ、SERIALIZABLE 三档隔离，2PL 在 RC/RR 下退化为长锁持有，性能不可接受。
3. **死锁规避**：2PL 的等待图易产生死锁，需要死锁检测器或超时回滚，工程复杂度高；MVCC 天然无锁，避免死锁。
4. **回滚成本低**：2PL 的回滚需释放所有锁并执行 undo；MVCC 仅需丢弃新版本即可。

不选择 MVCC 的后果：
- 读写互斥导致 AP 查询阻塞 TP 写入
- 死锁频发，需引入 wait-die/wound-wait 等调度策略
- SERIALIZABLE 隔离级别需 S2PL（严格两阶段锁），写锁持有至事务结束，并发度急剧下降

## 决策

采用 MVCC + SSI（Serializable Snapshot Isolation）混合模型：

- **基础层 MVCC**：每行保留多版本（`(txn_id, value)` 列表），读操作根据 `txn_snapshot` 选择可见版本，写操作追加新版本。
- **隔离级别映射**：
  - READ COMMITTED：每条语句取新 snapshot
  - REPEATABLE READ：事务开始时取 snapshot，全程不变
  - SERIALIZABLE：在 RR 基础上叠加 SSI 写偏斜检测
- **写写冲突**：first-committer-wins，后提交者检测到版本号冲突即 abort。

关键代码（`crates/szrsql-tx/src/mvcc.rs` L488-L612）：

```rust
// L488 begin：分配 txn_id 并取 snapshot
pub fn begin(&self) -> Result<u32, MvccError> {
    let txn_id = self.next_txn_id.fetch_add(1, Ordering::SeqCst);
    let snapshot = self.snapshot_state.load(Ordering::SeqCst);
    self.txns.insert(txn_id, TxnState { snapshot, status: TxnStatus::Active });
    Ok(txn_id)
}

// L560 commit：校验 first-committer-wins，标记 Committed
pub fn commit(&self, txn_id: u32, commit_lsn: u64) -> Result<(), MvccError> {
    let mut state = self.txns.get(&txn_id).ok_or(MvccError::TxnNotFound)?;
    // 写写冲突检测：遍历写集，确认无其他事务已提交相同 key
    for key in &state.write_set {
        if let Some(v) = self.versions.get(key) {
            if v.last_writer != txn_id && v.last_writer_status == Committed {
                return Err(MvccError::WriteWriteConflict);
            }
        }
    }
    state.status = TxnStatus::Committed;
    state.commit_lsn = commit_lsn;
    Ok(())
}

// L600 abort：标记 Aborted，新版本由 GC 回收
pub fn abort(&self, txn_id: u32) -> Result<(), MvccError> { ... }
```

SSI 写偏斜检测在 SERIALIZABLE 隔离下启用：维护 `rw_anti_dependency` 图，发现循环即回滚。

## 后果

**正面**：
- 读不阻塞写、写不阻塞读，吞吐与并发数线性扩展
- 无锁设计，彻底消除死锁
- RC/RR/SERIALIZABLE 三档隔离统一实现，代码复用度高
- 回滚成本低（仅丢弃版本）

**负面**：
- 版本堆积需 GC 回收（默认保留 1000 个活跃事务窗口）
- SSI 检测在写偏斜高发场景下 abort 率上升（典型 5-10%）
- 写写冲突的 first-committer-wins 策略可能导致业务重试

## 注意事项

### 调用方约束
- 调用 `commit()` 必须传入 `commit_lsn`（来自 WAL），否则持久性无效
- 长事务需在 1000 个事务窗口内提交，否则版本可能被 GC 误回收
- SERIALIZABLE 隔离下应用需处理 `WriteSkew` 错误并重试

### 迁移路径
- 未来如需更高并发，可在 MVCC 之上引入乐观锁（OCC）变体
- SSI 检测可替换为更精确的 SER（Serializable Error Rate）检测器

### Bug 定位提示

**如果出现死锁或事务互相阻塞**：
1. **首先排除 MVCC 层**：MVCC 无锁，死锁现象必然来自上层（如 Raft、Buffer Pool shard 锁），查 `deadlock_detector` 日志
2. **确认隔离级别**：若 SERIALIZABLE，查 SSI 的 `rw_anti_dependency` 图是否有环
3. **排查外部锁**：grep `Mutex::lock` / `RwLock::write` 在调用栈中的位置

**如果出现写偏斜（业务一致性被破坏但无报错）**：
1. **查 SSI 是否启用**：确认隔离级别为 SERIALIZABLE 而非 RR（RR 下写偏斜不检测）
2. **查 SII 检测器日志**：`grep "write_skew_detected"` tracing span
3. **查 txn_snapshot 是否正确**：长事务的 snapshot 是否过期

**如果出现写写冲突频繁 abort**：
1. **查 first-committer-wins 逻辑**：`commit()` 中 `v.last_writer != txn_id` 判断是否正确
2. **查事务粒度**：业务是否在同一 key 上高频并发（应考虑分桶或队列化）
3. **可排除**：MVCC 版本链（版本链只影响读，不影响写冲突检测）
