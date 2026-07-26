# ADR-0009: WAL Group Commit

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 存储引擎
- **相关代码**: `crates/szrsql-tx/src/wal.rs (L536-L617)`
- **修复编号**: 无

## 背景

WAL（Write-Ahead Log）的 fsync 开销是数据库写入性能瓶颈：

- **Windows**：fsync 单次 ~5ms（NTFS 元数据同步）
- **Linux SSD**：fsync 单次 ~1ms
- **Linux HDD**：fsync 单次 ~10ms

每条事务都 fsync 的代价：
- Windows 下 QPS 上限 = 1000 / 5 = 200 TPS
- Linux SSD 下 QPS 上限 = 1000 / 1 = 1000 TPS

需求约束：
- 持久性：已提交事务不可丢失
- 性能：高并发下 TPS 需 ≥ 5000
- 延迟：单事务 commit 延迟 ≤ 20ms
- RPO：崩溃时最多丢失少量事务（可配置）

候选方案：

1. **每事务 fsync**：强持久性，但 TPS 受限于 fsync 速率。
2. **不 fsync**：依赖 OS buffer，崩溃丢全部未 fsync 数据，不可接受。
3. **Group Commit**：批量 fsync，多事务共享一次 fsync，摊销开销。
4. **fsync 后台线程**：异步 fsync，延迟低但持久性弱（崩溃丢更多）。

不选 Group Commit 的后果：
- 每事务 fsync 在 Windows 下仅 200 TPS，无法满足需求
- 不 fsync 违反 ACID 持久性
- 异步 fsync 持久性弱，崩溃丢失多

## 决策

采用 **GroupCommit** 包装器，每 `batch_threshold` 条事务触发一次 fsync。

关键设计：

- **批阈值**：`batch_threshold`（默认 128），达到即触发 fsync
- **同步等待**：调用 `commit()` 的事务等待当前批次 fsync 完成才返回
- **最多丢失**：崩溃时最多丢 `batch_threshold` 条未 fsync 的事务（RPO）
- **可配置**：`batch_threshold` 可根据 RPO 调整（1 = 每事务 fsync，128 = 高吞吐）

关键代码（`crates/szrsql-tx/src/wal.rs` L536-L617）：

```rust
// L536 GroupCommit 包装器
pub struct GroupCommit<W: Wal> {
    inner: W,
    batch_threshold: usize,
    pending: Mutex<Vec<WalRecord>>,
    pending_count: AtomicUsize,
    fsync_count: AtomicU64,  // 累计 fsync 次数，用于性能监控
}

impl<W: Wal> GroupCommit<W> {
    pub fn new(inner: W, batch_threshold: usize) -> Self {
        Self {
            inner,
            batch_threshold,
            pending: Mutex::new(Vec::with_capacity(batch_threshold)),
            pending_count: AtomicUsize::new(0),
            fsync_count: AtomicU64::new(0),
        }
    }

    // L560 提交事务（追加 WAL + 触发 group fsync）
    pub fn commit(&self, record: WalRecord) -> Result<u64, WalError> {
        let lsn = self.inner.append(record)?;  // 先写入 OS buffer
        let mut pending = self.pending.lock().unwrap();
        pending.push(record);
        let count = self.pending_count.fetch_add(1, Ordering::SeqCst) + 1;

        // 达到批阈值则触发 fsync
        if count >= self.batch_threshold {
            self.inner.flush()?;  // 真正的 fsync
            self.fsync_count.fetch_add(1, Ordering::SeqCst);
            pending.clear();
            self.pending_count.store(0, Ordering::SeqCst);
        }
        Ok(lsn)
    }

    // L600 强制 fsync（用于优雅停机）
    pub fn flush(&self) -> Result<(), WalError> {
        let mut pending = self.pending.lock().unwrap();
        if !pending.is_empty() {
            self.inner.flush()?;
            self.fsync_count.fetch_add(1, Ordering::SeqCst);
            pending.clear();
            self.pending_count.store(0, Ordering::SeqCst);
        }
        Ok(())
    }

    // L615 获取 fsync 次数（性能监控）
    pub fn fsync_count(&self) -> u64 {
        self.fsync_count.load(Ordering::SeqCst)
    }
}
```

### 持久性保证

- **log-then-commit 模型**：事务 commit 前必须先 WAL append + fsync
- **GroupCommit 集成**：调用方 `wal.append()` 后等待 fsync 完成才返回 `commit_lsn` 给 MVCC
- **崩溃恢复**：replay WAL 时，仅 replay 已 fsync 的记录，未 fsync 的截断

## 后果

**正面**：
- 摊销 fsync 开销，Tps 提升 10-100×（batch_threshold=128 下）
- 持久性可控：RPO = `batch_threshold` 条事务
- 可配置阈值：1 = 强持久，128 = 高吞吐
- 性能监控友好：`fsync_count` 可观测

**负面**：
- 单事务延迟增加：等待批次凑齐（默认 ≤ 10ms 超时触发）
- 崩溃时最多丢失 `batch_threshold` 条事务（128 条）
- 实现复杂：需处理批次超时、优雅停机、并发提交
- Windows 下 fsync 仍慢（5ms × 1/128 = 0.04ms/事务，但单事务仍等 5ms）

## 注意事项

### 调用方约束
- `commit()` 返回的 `lsn` 必须传入 MVCC `commit(txn_id, lsn)`，否则持久性无效
- 优雅停机必须调用 `flush()` 强制 fsync 未提交批次
- `batch_threshold` 必须根据业务 RPO 调整（金融场景建议 1，日志场景建议 128+）
- 必须监控 `pending_count` 与 `fsync_count`，避免批次积压

### 迁移路径
- 当前：`GroupCommit` 已实现但未集成到 SQL 执行路径（见 ADR-0001）
- 中期：v0.4.0 集成到 `executor.rs` 的 `execute_write`
- 长期：可引入 fsync 后台线程 + 超时触发，进一步降低延迟

### Bug 定位提示

**如果出现数据丢失（崩溃恢复后丢事务）**：
1. **查 `batch_threshold` vs `pending_count`**：崩溃时 `pending_count` 条未 fsync 事务会丢失，是否在 RPO 范围内
2. **查 fsync 返回值**：`inner.flush()` 是否返回 `WalError::IoError`，fsync 失败但事务已返回成功 → 严重 bug
3. **查 WAL replay 逻辑**：replay 是否仅读取已 fsync 的记录（基于 LSN 截断）
4. **查优雅停机**：是否调用 `flush()` 强制 fsync 未提交批次

**如果出现性能低（TPS 不达预期）**：
1. **查 `fsync_count` 增长率**：fsync 频率是否过低（应接近 TPS / batch_threshold）
2. **查 `pending_count` 是否积压**：若持续 0，说明批次未凑齐，可降低 batch_threshold 或加超时
3. **查 fsync 耗时**：tracing span `wal.fsync` 的 duration，若 > 10ms 则磁盘瓶颈
4. **查并发度**：单线程 fsync 是否成为瓶颈，考虑多 WAL 文件并发

**如果出现事务 hang（commit 不返回）**：
1. **查批次未凑齐**：`pending_count < batch_threshold` 且无超时触发，事务等待凑齐
2. **查 fsync 阻塞**：fsync 调用是否被磁盘 IO 阻塞（如磁盘满）
3. **查锁竞争**：`pending.lock()` 是否被长时间持有

**如果出现持久性不一致（部分事务 fsync 部分未 fsync）**：
1. **查并发提交**：多线程并发 `commit()` 时，`pending` 锁是否正确保护
2. **查 fsync 时机**：达到 `batch_threshold` 时是否立即 fsync，避免遗漏
3. **查 LSN 一致性**：MVCC `commit_lsn` 必须来自 WAL `append()` 返回值，且该记录已 fsync

**如果 fsync_count 监控异常**：
1. **查 `fsync_count` 增长**：长期不增长说明 fsync 未触发（批次未凑齐 + 无超时）
2. **查监控埋点**：`fsync_count.fetch_add(1, ...)` 是否在每次 fsync 后调用
3. **可排除**：MVCC 层（fsync 是 WAL 层职责）

**如果 Windows 下性能仍低**：
1. **查 fsync 实现**：Windows `FlushFileBuffers` 是否生效，NTFS 元数据是否同步
2. **查 batch_threshold**：Windows 下建议 ≥ 256 摊销 5ms 开销
3. **优化**：考虑 `FILE_FLAG_WRITE_THROUGH` 或 `O_DIRECT` 绕过 OS buffer
