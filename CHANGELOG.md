# Changelog

All notable changes to SzRSQL are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(see [VERSIONING.md](VERSIONING.md) for SzRSQL-specific compatibility rules).

## [Unreleased]

### Added

- P4-1 CDC：全量快照 + 增量连接（`ReplicationTask::snapshot_lsn: AtomicU64`，`start_task_with_snapshot` API）
- P4-2 CDC：Schema 变更同步（`SchemaChangeEvent` / `SchemaChangeObserver` + 多方言 DDL 生成器，支持 CREATE/ALTER ADD COLUMN/DROP TABLE，DropColumn 保守策略）
- P4-3 CDC：性能基准（6 项 benchmark，debug 模式 TPS 3.1M，端到端平均延迟 185ns，p99 300ns，DDL 同步开销 +5.65%）
- P4-4 CDC：真实 PG 集成测试（6 项 integration_pg 测试）

### Changed

- **文档诚实化修正**（基于全面审计报告 §9 P0-DOC-1..3）：
  - README PostgreSQL 兼容率 89.6% → 100%（与兼容性报告一致）
  - CHANGELOG 停留在 0.1.0 → 对齐至 v1.0.0-rc.1
  - crate 数量 14 → 22，测试数量 402 → ~8,850

- **运行时实际架构澄清**：当前运行时为 `Vec<Row>` 内存表 + JSON 快照 + WAL（Commit/Abort 标记）+ 表级 snapshot/restore + 多方言协议兼容层。B+Tree/BufferPool/MVCC/Raft/Percolator 代码完整但 `#![allow(dead_code)]`，生产代码 0 调用，需后续核心重构接入。

### Fixed

- **P0-SQL-1..6**：补全 CREATE INDEX / DROP INDEX / DROP TABLE / TRUNCATE / COPY / LISTEN-NOTIFY 执行方法（executor.rs）
- **P0-SQL-7**：触发器 WHEN 子句接入 ExprEvaluator 对 NEW/OLD 求值（trigger.rs）
- **P0-SQL-8**：UDF 接入 ExprEvaluator（thread_local + RAII guard；udf.rs `call_counter` 改为 `AtomicU64`，`call`/`next_call_id` 改为 `&self`；expr.rs `EvalContext` 增加 `try_call_udf`；executor.rs 持有 `Option<Arc<UdfRegistry>>`）
- **P0-PG-1..5**：pgwire session.rs 5 个 DDL NO-OP 接入 executor（CreateIndex/DropIndex/CreateView/DropView/RefreshMaterializedView）
- **P0-PG-6**：SET TRANSACTION ISOLATION LEVEL 记录到 session_state（注：实际隔离仍为表级 snapshot/restore，需 MVCC 接入后才能完整生效）
- **P0-PG-7**（部分）：pg_sequence 系统表真实化（navicat.rs 新增 `pg_sequence(catalog)`，ManagedCatalog 增加序列存储；3 个测试通过）
- **P0-PG-8**（部分）：新增 `CredentialStore` 持久化结构（auth.rs JSON save/load，5 个测试通过；待接入启动流程）
- **P0-PG-10**：COPY FROM 约束校验接入（FK/CHECK/ENUM）
- **P0-TX-2**：WAL Replayer 接入启动流程（main.rs 启动时 `WalReplayer::replay_all`；改用 open 而非 create_new 避免截断）
- **DROP TABLE IF EXISTS**：检查 local/shared/catalog 三层存储
- **DROP TABLE CASCADE / TRUNCATE CASCADE**：递归收集 FK 依赖表级联删除/清空
- **COMMENT ON COLUMN**：要求列名，缺失时返回错误
- **8 处 unreachable!()**：替换为 `SessionError::InvalidStatement`
- **ALTER TABLE unwrap()**：列迁移路径改为 `ok_or_else` 返回错误
- **FLASHBACK history**：commit_transaction 持久化事务快照到 TransactionHistory

## [1.0.0-rc.1] - 2026-07-29

### Added

- **多方言协议层**（L2 协议级兼容）：
  - PostgreSQL：pgwire v3.0 + SCRAM-SHA-256 + 扩展查询协议 + TLS 1.3 + COPY FROM/TO + LISTEN/NOTIFY
  - MySQL：Wire Protocol v10 + mysql_native_password + SQL 归一化
  - SQL Server：TDS 协议（基础）
  - Oracle：TNS 协议桥接 + SQL 方言转换
  - SQLite：文件格式协议级兼容（基础）

- **Phase 6.3 Navicat 兼容性增强**：
  - MySQL 协议 SQL 归一化（反引号 → 双引号、`LIMIT offset, count` → `LIMIT count OFFSET offset`、`SELECT @@SESSION.x` / `SELECT @@GLOBAL.x`、`SHOW DATABASES` / `DESC table` / `SHOW COLUMNS FROM table` / `INFORMATION_SCHEMA.KEY_COLUMN_USAGE`）
  - Navicat SET 语句归一化（9 种变体）
  - 跨会话/跨协议共享表存储（`MysqlServer` 与 `PgwireServer` 共享同一实例）
  - 带 schema 限定的表查询修复
  - 系统表 JOIN 查询 Describe 列数匹配
  - TIMESTAMP 类型解析增强（空格分隔符 + 小数秒）

- **Phase 6.2 连接空闲超时清理**：
  - `--connection-idle-timeout <u64>` CLI 参数（默认 300 秒，0 = 禁用）
  - 4 种协议全部覆盖（PG/MySQL/TDS/Oracle）
  - 超时自动回滚未提交事务 + 释放所有行锁
  - 防止客户端 kill -9 / Stop-Process / 网络中断导致的会话死锁

- **Phase 4.5.8-4.5.10 HTTP 管理**：
  - `/healthz`、`/readyz`、`/metrics`（Prometheus 格式）
  - `/api/v1/sessions`、`/api/v1/cancel/{pid}`、`/api/v1/backup`、`/api/v1/config/reload`
  - Bearer token 鉴权

- **Phase 4.13 守护进程**：`--daemon` Unix 双 fork + setsid + PID 文件 RAII
- **Phase 4.12 信号处理**：SIGTERM 优雅关闭 / SIGINT 立即关闭 + Crash Handler（panic hook + backtrace + WAL LSN 占位）
- **Phase 4.11 优雅关闭**：SIGTERM → Draining → 排空活跃连接 → Closed
- **Phase 4.10 Rust sqlx 驱动验证**：pgwire 协议兼容性
- **Phase 4.9 psql 互操作验证**：简单查询、扩展查询、事务、COPY
- **Phase 4.8 COPY FROM/TO**：CSV/TEXT 文件批量导入导出
- **Phase 4.7 元数据查询**：pg_tables/pg_indexes + information_schema
- **Phase 4.6 错误码 + 通知**：LISTEN/NOTIFY/UNLISTEN 跨会话
- **Phase 4.5 TLS 1.3**：rustls 集成 + SSLRequest 协商
- **Phase 4.4 认证**：trust + SCRAM-SHA-256（RFC 5802 + RFC 7677）
- **Phase 4.3 扩展查询协议**：Parse/Bind/Execute/Describe/Close/Sync/Flush
- **Phase 4.2 简单查询协议**：接入真实 SQL 执行器
- **Phase 4.1 pgwire 服务器启动**：监听 5432，启动消息握手

### Security

- SCRAM-SHA-256 认证使用恒定时间比较防止时序攻击
- TLS 1.3 默认拒绝客户端证书验证（`sslmode=verify-full` 时强制验证）
- TDE 透明加密（AES-256-CTR）+ 列级加密（AES-256-GCM）
- SQL 注入检测 + 防火墙 + 审计日志 + 脱敏

### Notes

- **Release Candidate 阶段**：v1.0.0-rc.1，功能基本完整，待 Jepsen + 7×24h 长稳测试通过后发布 v1.0.0 GA
- **运行时实际架构**：详见 README「数据持久化」与「分布式事务（Raft + Percolator）」章节，以及 `docs/全面审计报告.md` §十
- **已知限制**：B+Tree/MVCC/Raft/Percolator 代码完整但未接入运行时（生产代码 0 调用），运行时为内存表 + JSON 快照；MySQL Prepared Statement / Oracle TTC / SQLite B-tree / TDS RPC 等方言能力为简化实现

## [0.1.0] - 2026-07-20

### Added

- 项目骨架：14 个 workspace crate（types、storage、tx、cdc、sql、catalog、protocol、optimizer、ai、security、scheduler、replication、dist、pgcompat、bin）
- Phase 1-3：存储引擎（B+Tree、Buffer Pool、Page）、事务引擎（MVCC、WAL、Lock）、SQL 引擎（Parser、AST、Executor）
- Phase 4.1-4.13：pgwire 协议层完整实现
- 402 单元/集成测试通过（3 ignored）

### Notes

- Alpha 阶段版本，不承诺兼容性
- 预期 GA 版本 `1.0.0`（Phase 8 完成后，Jepsen + 7×24h 长稳测试通过）

[Unreleased]: https://github.com/szrsql/szrsql/compare/v1.0.0-rc.1...HEAD
[1.0.0-rc.1]: https://github.com/szrsql/szrsql/releases/tag/v1.0.0-rc.1
[0.1.0]: https://github.com/szrsql/szrsql/releases/tag/v0.1.0
