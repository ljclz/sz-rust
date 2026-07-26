# SzRSQL ADR 索引

> 架构决策记录（Architecture Decision Record）索引。
> 规范见 `docs/ADR与生产Bug定位规范.md`。

## 状态统计

| 状态 | 数量 |
|------|------|
| Accepted | 10 |
| Proposed | 0 |
| Superseded | 0 |
| Deprecated | 0 |

## ADR 列表

| 编号 | 标题 | 状态 | 决策类型 | 日期 |
|------|------|------|---------|------|
| [0001](0001-persistence-model.md) | 持久性模型：当前状态与 log-then-commit 迁移路径 | Accepted | 存储引擎 | 2026-07-24 |
| [0002](0002-mvcc-over-2pl.md) | MVCC 优先于 2PL 作为并发控制原语 | Accepted | 并发原语 | 2026-07-24 |
| [0003](0003-raft-consensus.md) | Raft 作为分布式共识算法 | Accepted | 分布式共识 | 2026-07-24 |
| [0004](0004-percolator-distributed-tx.md) | Percolator 作为分布式事务模型 | Accepted | 分布式事务 | 2026-07-24 |
| [0005](0005-hlc-clock.md) | HLC 作为因果一致性时钟方案 | Accepted | 时钟方案 | 2026-07-24 |
| [0006](0006-range-sharding.md) | Range-based 分片策略 | Accepted | 分片策略 | 2026-07-24 |
| [0007](0007-identifier-escaping.md) | SQL 标识符转义防注入 | Accepted | SQL注入防护 | 2026-07-24 |
| [0008](0008-page-size-16kb.md) | Page 大小固定为 16KB | Accepted | 存储引擎 | 2026-07-24 |
| [0009](0009-wal-group-commit.md) | WAL Group Commit 批量 fsync 策略 | Accepted | 存储引擎 | 2026-07-24 |
| [0010](0010-buffer-pool-sharded-lru.md) | 缓冲池分片 LRU 设计 | Accepted | 资源限制 | 2026-07-24 |

## 使用说明

1. **新增 ADR**：复制 `template.md`，按编号递增创建新文件
2. **修改 ADR**：已 Accepted 的 ADR 内容不可变；新决策写新 ADR 并标注 `Superseded by ADR-XXXX`
3. **Bug 定位**：grep ADR 中的 "Bug 定位提示" 段，对照现象排除设计限制
4. **覆盖率检查**：每季度执行覆盖率审计，识别 ADR + tracing 双盲区
