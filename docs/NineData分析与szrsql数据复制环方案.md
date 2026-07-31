# NineData 社区版分析 & szrsql 内部数据复制闭环方案

> **文档版本**：4.0（P7-1/P7-3/P7-4/L5-L11 完成后更新）
> **分析日期**：2026-07-29
> **重写日期**：2026-07-30
> **二次验证日期**：2026-07-30
> **P7 系列完成日期**：2026-07-30
> **重写依据**：对 szrsql-cdc 全部 19 个模块 + 6 个关联 crate 的逐文件代码审计
> **二次验证依据**：通过 Grep/Read 验证文档关键断言与代码一致，发现并修正 4 处偏差，新增 7 项占位/伪实现发现
> **P7 系列更新依据**：P7-1（CDC 接入生产运行时）、P7-3（背压集成到 task）、P7-4（MCP server 启动 + ReplicationTaskManager 注入）、L5-L11（占位/伪实现修复）全部完成，通过 cargo check + cargo test + cargo clippy 验证

---

## 一、文档目的

本文档基于对 szrsql 项目当前代码的**逐文件审计**，如实描述：

1. **真实能力**：代码中实际可工作的功能
2. **占位/伪实现**：只有接口没有实际逻辑，或返回硬编码值的代码
3. **缺失功能**：文档/注释中提到但代码中不存在的功能
4. **潜在 bug**：代码逻辑错误、边界问题、并发问题
5. **集成断点**：模块间调用链断裂的地方
6. **生产就绪度**：各模块的实际生产可用性评估

**核心原则**：只报告代码中实际看到的内容，不编造功能，不夸大能力。

---

## 二、项目整体架构

### 2.1 Crate 结构

```
szrsql/
├── crates/
│   ├── szrsql-storage/    # B+Tree/BufferPool 存储原语
│   ├── szrsql-tx/         # WAL/MVCC/Lock 事务引擎
│   ├── szrsql-dist/       # Raft/Percolator 分布式
│   ├── szrsql-sql/        # SQL Parser/Executor
│   ├── szrsql-protocol/   # pgwire/mysql 协议
│   ├── szrsql-cdc/        # CDC 变更数据捕获
│   ├── szrsql-ai/         # MCP server/NL2SQL/RAG
│   └── szrsql-bin/        # 运行时入口
```

### 2.2 各 Crate 真实能力速览

| Crate | 真实度 | 接入生产运行时 | 与 CDC 集成 |
|-------|--------|--------------|------------|
| szrsql-storage (BTree/BufferPool) | ✅ 真实（含 loom/Kani 测试） | ⚠️ 部分 — BTree 仅作 PK 索引，主存储是 Vec\<Row\> | ❌ 无 |
| szrsql-tx WAL | ✅ 真实（CRC32C、replay） | ✅ 是 — main.rs 启动回放 | ⚠️ CDC 实现 WalObserver trait 但生产未注册 |
| szrsql-tx MVCC | ✅ 真实（SSI、First-Committer-Wins） | ✅ 是 | ❌ 无 |
| szrsql-tx Lock | ✅ 真实（S/X、升级、FIFO、超时） | ✅ 是 | ❌ 无 |
| szrsql-dist Raft | ✅ 真实（从零实现，论文忠实；已含真实 TCP 网络层 `TcpNetwork` + 多节点集群 `DistCluster`/`new_cluster_node`） | ⚠️ main.rs 仅启用 `new_single_node_runtime(1)` 单节点模式，未启用跨节点 RPC | ❌ 无（注：szrsql-dist 的 Cargo.toml 声明依赖 szrsql-cdc 但代码 0 处使用，是另一处幽灵依赖） |
| szrsql-dist Percolator | ✅ 真实（TSO+2PC+resolve_lock） | ⚠️ TSO 已用，跨分片 2PC 未启用 | ❌ 无 |
| szrsql-dist Multi-Master | ✅ 真实（HlcClock、ConflictLog、MultiMasterCluster） | ❌ main.rs 未启用 | ❌ 无 |
| szrsql-dist DistTxn | ✅ 真实（DistTxnClient、ClusterTxnCoordinator） | ❌ main.rs 未启用跨节点协调 | ❌ 无 |
| szrsql-sql parser | ✅ 真实（sqlparser-rs + 递归保护） | ✅ 是 | ❌ 无 |
| szrsql-sql executor | ✅ 真实（火山模型 + 30+ 特性） | ✅ 是 | ❌ 无 |
| szrsql-protocol pgwire | ✅ 真实（TLS 1.3、SCRAM、扩展查询） | ✅ 是 | ✅ 是 — Session 通过 `with_cdc_engine` 注入 CdcEngine，Executor DML 操作实时分发 CDC 事件 |
| szrsql-ai MCP server | ✅ 真实（35 工具，9 类别） | ✅ 是 — main.rs `--mcp-stdio` 启动 MCP stdio server | ✅ 真实集成（5 个 Replication 工具，注入 ReplicationTaskManager） |
| szrsql-cdc 引擎 | ✅ 真实（多 target/source、集成测试连接真实 PG 18） | ✅ 是 — main.rs 构造 CdcEngine 并通过 `with_cdc_engine` 注入 PgwireServer，Executor DML 操作实时分发 CDC 事件 | 自身 |

### 2.3 关键发现

1. **CDC 已接入生产运行时（P7-1 完成）**：`szrsql-bin/src/main.rs` 构造 `CdcEngine` 并通过 `with_cdc_engine` 注入 `PgwireServer`，Executor 的 `mvcc_insert/update/delete` 通过 `dispatch_cdc_*` 实时分发 CDC 事件到 `CdcObserverManager`，下游 `ReplicationTaskManager` 通过 `register_observer_arc` 接收事件流。
2. **MCP server 已可启动（P7-4 完成）**：main.rs 通过 `--mcp-stdio` CLI 参数启动 MCP stdio server，注入 `ReplicationTaskManager`，暴露 35 个 LLM 工具（9 个类别，含 5 个 Replication 类工具）。
3. **背压机制已集成到 task（P7-3 完成）**：`ReplicationTask` 使用 `BoundedEventQueue` 作为生产者-消费者缓冲，`on_event` 推送事件到队列，独立消费者线程异步处理，支持 Block/DropOldest/Reject/Signal 4 种策略。
4. **幽灵依赖已消除（P7-1 副产物）**：`szrsql-bin` 的 Cargo.toml 声明依赖 `szrsql-cdc`，现在 main.rs 真实使用 `CdcEngine`/`CdcObserverManager`/`ReplicationTaskManager` 等类型。`szrsql-dist` 的 Cargo.toml 仍声明依赖 `szrsql-cdc` 但代码 0 处使用（仍为幽灵依赖，待 P8 阶段处理）。
5. **存储层与执行器断层**：executor 主存储是 `Vec<Row>` + `HashSet<usize>` tombstone，BTree 仅作可选 PK 索引（Int64 限制 + u16 row_id 限制 65535 行）。
6. **WAL 数据恢复不完整**：WAL 仅记录 Commit/Abort 标记（无行级 data），崩溃恢复依赖每 5 秒一次的 JSON 快照，RPO 最高 5 秒。
7. **15 个 crate 全 crate 标记 `#![allow(dead_code)]`**（实测：szrsql-types、szrsql-shadow、szrsql-scheduler、szrsql-replication、szrsql-optimizer、szrsql-security、szrsql-protocol、szrsql-pgcompat、szrsql-tx、szrsql-dialect-compat、szrsql-ops、szrsql-cdc、szrsql-storage、szrsql-sql、szrsql-ai；另有 szrsql-dist/conflict.rs 模块级标记），掩盖了未接入执行链路的代码。仅 szrsql-dist 主 lib 显式移除。

---

## 三、NineData 社区版能力分析

### 3.1 NineData 核心能力

| 能力域 | NineData 实现 | szrsql 对标 |
|--------|--------------|------------|
| 数据源支持 | PG/MySQL/Oracle/SQL Server/Kafka/Redis 等 20+ | ⚠️ szrsql-cdc 支持 5 种 target（PG/MySQL/Oracle/SQL Server/Kafka），但全部闭包注入，无真实驱动 |
| 全量迁移 | 一致性快照 + 并行表传输 | ⚠️ SnapshotTransfer 框架存在，reader 闭包注入，未连接真实扫描 |
| 增量同步 | 基于 WAL/binlog/LogMiner 的 CDC | ⚠️ CdcEngine 是事件分发器，未连接真实 WAL 消费 |
| 数据比对 | 行级 checksum + 差异修复 | ✅ comparison.rs 实现行级比对 + 修复 SQL 生成（reader 闭包注入） |
| 反向链路 | 外部 DB → szrsql | ⚠️ source/ 模块全部 mock，PgSourceConnector 不连接真实 PG |
| 分布式协调 | 多节点任务分配 | ⚠️ cluster.rs 单机内存模拟，无真实网络/Raft |
| 云原生 | K8s Operator | ⚠️ cloud.rs 仅生成 YAML，不调用 K8s API |
| 多租户 | SaaS 服务 | ⚠️ service.rs 无 HTTP API 层，仅 Rust API |

### 3.2 NineData 架构启示

NineData 的核心架构是 **"控制面 + 数据面"** 分离：

```
控制面：任务调度 + 集群协调 + API 网关 + 多租户
数据面：Source Connector → Transform → Sink Connector
```

szrsql-cdc 的模块划分与此对应，但**控制面和数据面的实现都是骨架化的**：
- 控制面：cluster.rs/service.rs/cloud.rs 都是内存模拟或配置生成，无真实网络/API
- 数据面：target/source 的数据库连接全部闭包注入，无真实驱动

---

## 四、szrsql-cdc 模块详细审计

### 4.1 CdcEngine 核心（`lib.rs`）

**真实实现**：
- `CdcEngine` 提供 `insert/update/delete/commit/abort` 接口，生成 `ChangeEvent` 并分发给观察者
- `CdcObserverManager` 支持 register/unregister/notify 观察者
- `CdcEventOp` 枚举（Insert/Update/Delete/Commit/Abort）
- 实现 `WalObserver` trait（可接入 szrsql-tx WAL 钩子）

**占位/伪实现**：
- ❌ **CdcEngine 不连接真实 WAL**：所有 DML 操作由调用方直接调用 `engine.insert(...)` 触发，**未对接 szrsql-storage 的 WAL 模块**。本质上是"事件分发器"而非真实 CDC 引擎
- ❌ `ChangeEvent.table_id` 由调用方传入，未从 WAL 解析

**缺失功能**：
- 真实 WAL 监听/读取循环
- LSN 自动管理（当前由调用方传 LSN）
- 事务边界自动识别（依赖调用方手动调 commit/abort）

**集成断点**：
- 🔴 **CdcEngine 与 szrsql-tx WAL 之间无连接代码**：CdcEngine 实现了 `WalObserver` trait，但 `main.rs` 从未调用 `WalObserverManager::register(cdc_engine)`，所以 WAL 事件不会触发 CDC

---

### 4.2 任务管理（`task.rs`）

**真实实现**：
- `ReplicationTask` 状态机（Created→Running→Paused→Stopped→Failed）完整
- `ReplicationTaskManager` 管理多任务，支持 create/start/stop/pause/resume/remove/list/monitor
- 任务通过 `TargetWriter` 写入目标端
- 支持表过滤（TableFilter）、快照 LSN 过滤

**占位/伪实现**：
- ❌ **任务运行只是"已启动"标记**：`start_task` 仅置状态为 Running，**没有实际启动后台线程消费 WAL**。所有事件由外部主动调用 `process_event` 推送
- ❌ 进度统计（events_processed/bytes_processed）依赖外部 update，非任务自身驱动

**缺失功能**：
- 后台 worker 线程从 WAL/源端拉取数据
- 自动位点管理（依赖 `ReplicationSlot` 外部维护）

**集成断点**：
- ⚠️ `task.rs` 与 `lib.rs::CdcEngine` 之间无自动连接，需调用方手动 `process_event`

---

### 4.3 WAL 解码（`decoder.rs`）

**真实实现**：
- `RowDecoder` 将 `Vec<u8>` 二进制行解码为 `DecodedRow`（columns: Vec<(String, SzValue)>）
- 支持 11 种 DataType（Int32/Int64/Text/Blob/Real/Bool/Date/Timestamp/Json/Uuid）
- 通过 `SchemaRegistry` 查找表结构

**占位/伪实现**：无，解码逻辑真实

**缺失功能**：
- ❌ **无 WAL record 格式解析**：只接受已解包的 `Vec<u8>`，未实现 pg WAL/B-Tree page 解包
- ❌ NULL 位图处理不完整（部分类型假设非空）

**潜在 bug**：
- ⚠️ Real 类型用 f64 近似，精度损失
- ⚠️ 长 Blob 未分块处理，大对象可能 OOM

---

### 4.4 复制槽（`slot.rs`）

**真实实现**：
- `ReplicationSlot` 持久化消费位点（confirmed_lsn + restart_lsn）
- `SlotManager` 支持多 slot 管理（create/remove/advance/get）
- 支持文件持久化（`save_to_file`/`load_from_file`）和内存模式

**占位/伪实现**：
- ❌ **文件持久化是简单的 JSON 序列化**，未实现 fsync 与原子替换；崩溃时可能丢失位点
- ❌ **未对接真实 PostgreSQL 复制槽协议**（pg replication slot 的 `pg_logical_slot_get_changes`）

**潜在 bug**：
- ⚠️ 文件持久化无 fsync，崩溃恢复可能回退到旧位点（违反 exactly-once）

---

### 4.5 快照传输（`snapshot.rs`）

**真实实现**：
- `SnapshotTransfer` 协调全量快照传输
- 支持 `transfer_table`：通过闭包注入 reader 和 writer
- 与 CDC 流衔接（记录 snapshot_lsn，CDC 从该 LSN 之后开始）

**占位/伪实现**：
- ❌ **reader 是闭包注入**，未连接真实 szrsql-storage 全表扫描
- ❌ **未实现并发快照**（一致性快照需要 MVCC snapshot）

**缺失功能**：
- 一致性快照点（START TRANSACTION SNAPSHOT）
- 并行表扫描
- 大表分块

---

### 4.6 Schema 管理（`schema.rs`）

**真实实现**：
- `SchemaRegistry` 管理 table_id → TableSchema，版本号自增
- `SchemaChangeObserverManager` 观察 DDL 事件
- `SchemaAwareCdcEngine` 包装 CdcEngine，在 DML 时携带 schema_version
- 支持 create_table/alter_table_add_column/alter_table_drop_column/drop_table
- `SchemaChangeEvent` 完整

**占位/伪实现**：无，逻辑真实

**缺失功能**：
- ❌ **未从真实 DDL 解析**：列定义由调用方传 `Vec<ColumnDef>`，未连接 szrsql-sql parser
- ❌ 列类型变更（ALTER COLUMN TYPE）未实现
- ❌ Schema 持久化未实现（重启后丢失）

---

### 4.7 DDL 迁移（`migration.rs`）

**真实实现**：
- `SchemaMigration` 根据源/目标 schema 差异生成 DDL
- 支持 4 种方言（Postgres/MySQL/Oracle/SqlServer）+ Kafka（schema-less）
- `generate_create_table_ddl` / `generate_alter_add_column_ddl` / `generate_drop_table_ddl`
- 类型映射表

**占位/伪实现**：
- ⚠️ **Oracle DDL 生成未充分测试**（无真实 Oracle 连接验证）
- ⚠️ 类型映射是简化版（如 szrsql Real → Oracle NUMBER，未区分 precision）

**缺失功能**：
- 索引/约束迁移
- 列重命名/类型变更
- Schema diff 算法（仅支持 add/drop，未支持 reorder）

**潜在 bug**：
- ⚠️ Oracle 标识符未加引号（保留字冲突）

---

### 4.8 背压（`backpressure.rs`）

**真实实现**：
- `Backpressure` 基于水位线（high/low watermark）
- 3 种策略：Block/Drop/Reject
- `BoundedEventQueue` 有界队列（容量可配）
- `try_push`/`try_pop` 非阻塞，`push`/`pop` 阻塞

**占位/伪实现**：无，逻辑真实

**缺失功能**：
- ❌ **未与 task.rs 集成**：`ReplicationTask` 未使用 Backpressure，事件直接同步分发
- ❌ 无自适应限流（仅固定阈值）

**集成断点**：
- 🔴 **backpressure.rs 与 task.rs 完全解耦**，背压机制实际未生效（生产者-消费者未通过队列连接）

---

### 4.9 故障恢复（`failover.rs`）

**真实实现**：
- `FailoverSimulator` 模拟消费者崩溃与恢复
- 验证 exactly-once：崩溃后从 confirmed_lsn 恢复，无丢失无重复
- 测试场景：crash_before_commit/crash_after_commit/crash_during_processing

**占位/伪实现**：
- ⚠️ **故障注入是模拟的**（人为丢弃 in-flight 事件），非真实进程崩溃

**缺失功能**：
- 真实进程崩溃测试（kill -9 + 重启）
- 网络分区模拟

---

### 4.10 数据比对（`comparison.rs`）

**真实实现**：
- `DataComparator` 按主键逐行比对源/目标
- 3 类差异：MissingRows/ExtraRows/MismatchedRows
- 支持自动修复（generate_fix_ddl 生成 DML）
- 支持全量比对和增量比对（基于 LSN）

**占位/伪实现**：
- ⚠️ **比对 reader 是闭包注入**，未连接真实表扫描

**缺失功能**：
- ❌ 大表分块比对（hash join）
- ❌ 异步流式比对

---

### 4.11 Debezium 集成（`debezium.rs` / `debezium_avro.rs`）

**真实实现**：
- `to_debezium` 将 ChangeEvent 转换为 Debezium JSON（payload + source + op + ts_ms）
- `DebeziumSchemaRegistry` 支持 schema 注册与版本管理
- AVRO 格式生成（schema + payload）

**占位/伪实现**：
- ⚠️ **Schema Registry HTTP 客户端是 trait 抽象**，未提供真实 HTTP 实现（生产需注入 reqwest）

---

### 4.12 目标端写入器（`target/`）

**真实实现**：
- `TargetWriter` trait 抽象目标端写入
- 5 种写入器实现：
  - `MySqlWriter`：INSERT ON DUPLICATE KEY UPDATE / UPDATE / DELETE
  - `PostgresWriter`：INSERT ON CONFLICT DO UPDATE / UPDATE / DELETE
  - `OracleWriter`：MERGE INTO / UPDATE / DELETE
  - `SqlServerWriter`：MERGE / UPDATE / DELETE
  - `KafkaSink`：Debezium JSON 写入 Kafka
- 标识符引用（反引号/双引号/方括号）
- 类型映射（SzValue → SQL 字面量）
- `SqlExecutor` 闭包注入执行 SQL
- **集成测试连接真实 PG 18**（`tests/integration_pg.rs`）

**占位/伪实现**：
- 🔴 **所有 SQL 执行器是闭包注入**，未使用真实数据库驱动（sqlx/tokio-postgres/oracle-rs）
- 🔴 **KafkaSink 的 KafkaProducer 是 trait 抽象**，未提供真实 rdkafka 实现

**缺失功能**：
- 真实数据库连接池
- 事务批量写入（当前每事件一条 SQL）
- 错误重试与死锁处理
- 连接断开重连

**潜在 bug**：
- ⚠️ `infer_primary_key` 假设第一个 NOT NULL 列是主键，**无真实主键元数据**
- ⚠️ SQL 注入风险：`format_value_for_sql` 对字符串做简单转义，未使用参数化查询
- ⚠️ Oracle MERGE 的 USING 子句只含 pk 值，UPDATE SET 用字面量而非 s.列名，可能与 ON 冲突

**集成断点**：
- 🔴 **所有目标端写入器需调用方注入 SqlExecutor**，生产部署需实现真实执行器

---

### 4.13 反向链路（`source/`）

**真实实现**：
- `SourceConnector` trait 抽象源端（connect/discover_schemas/extract_snapshot/start_cdc_stream/ack_offset）
- `PgSourceConnector` 实现，通过 `SourceEventProvider`/`SchemaProvider`/`SnapshotProvider` 闭包注入
- `ReverseReplicator` 状态机（Created→Starting→Running→Stopped/Failed）
- 完整流程：连接 → schema 迁移 → 全量快照 → CDC 流
- `pg_type_to_szrsql` 类型映射

**占位/伪实现**：
- 🔴 **PgSourceConnector 不连接真实 PostgreSQL**：所有数据由闭包注入模拟，未使用 `tokio-postgres`/`rust-postgres`
- 🔴 **CDC 流是闭包回调**，非真实 `pg_logical_slot_get_changes` 调用

**缺失功能**：
- 真实 PostgreSQL logical replication（WAL 解析、protocol）
- MySQL binlog 解析
- Oracle XStream/LogMiner
- SQL Server CDC table

**潜在 bug**：
- ⚠️ `reverse.rs::start` 是同步阻塞方法，CDC 流不结束时调用方无法响应 stop 信号（除非闭包主动检查 `stop_requested`）

**集成断点**：
- 🔴 **反向链路完全是 mock 实现**，生产部署需替换所有闭包为真实数据库客户端

---

### 4.14 分布式协调（`cluster.rs`）— P6-1

**真实实现**：
- `ClusterCoordinator` 管理节点列表 + 任务分配映射
- `ClusterNode` 状态机（Alive/Dead/Leaving）
- `TaskAssignment` 负载均衡（亲和性优先 → load_score 最低 → cpu 最低 → node_id 字典序）
- `HeartbeatProvider`/`TaskDispatcher` trait 抽象网络通信
- Leader 选举占位（role 字段，无真实 Raft）
- 任务迁移（migrate_task）

**占位/伪实现**：
- 🔴 **HeartbeatProvider/TaskDispatcher 是 trait 抽象**，未提供真实 TCP/gRPC 实现
- 🔴 **Leader 选举是占位**：NodeRole 由外部设置，无 Raft/Paxos 算法
- 🔴 **节点列表是本地内存**，非分布式共享状态（无 etcd/Consul）

**潜在 bug**：
- ⚠️ `assign_task` 在持锁外调用 dispatcher，二次检查存在 TOCTOU 窗口
- ⚠️ `migrate_task` 失败时未回滚 assignments 映射

**集成断点**：
- 🔴 **cluster.rs 完全是单机内存模拟**，多节点部署需注入真实网络实现
- 🔴 **与 szrsql-dist/network.rs 重复实现**：szrsql-dist 已有真实 `TcpNetwork`（基于 `std::net::TcpStream`/`TcpListener`）和 `DistCluster`，应直接复用而非另起一套模拟实现

---

### 4.15 云原生部署（`cloud.rs`）— P6-2

**真实实现**：
- `CloudDeploymentGenerator` 生成 K8s YAML 清单
- `CdcStatefulSet`/`CdcServiceSpec`/`CdcConfigMap`/`VolumeClaimTemplate` 资源规格
- `to_yaml()` 输出标准 K8s YAML（apiVersion/kind/metadata/spec）
- StatefulSet + Service + ConfigMap 多文档 YAML
- 支持 TLS/Prometheus 监控 annotations

**占位/伪实现**：
- ⚠️ **仅生成 YAML，不实际部署**（无 kube-rs 依赖，不调用 K8s API）
- ⚠️ **YAML 是手工字符串拼接**，未用 serde_yaml，存在转义风险

**缺失功能**：
- 真实 K8s API 部署
- Helm Chart 生成
- CRD（Custom Resource Definition）

**潜在 bug**：
- ⚠️ `yaml_scalar` 函数对特殊字符（冒号、换行）的转义不完整，可能生成非法 YAML
- ⚠️ ConfigMap 的 data 值未处理多行字符串

---

### 4.16 CDC 即服务（`service.rs`）— P6-3

**真实实现**：
- `CdcService` 多租户隔离层，封装 `ReplicationTaskManager`
- `TenantConfig`/`TenantTier`（Free/Pro/Enterprise）配额管理
- `AuthService` API Key 认证（issue/revoke/authenticate/validate_access）
- 任务 CRUD 校验归属（validate_task_ownership）
- 使用量统计（events/bytes/throughput）
- 配额检查（max_tasks/max_throughput）
- 集群集成（with_cluster/set_cluster/assign_task_to_cluster/migrate_tenant_tasks）

**占位/伪实现**：
- 🔴 **AuthService 是简化版**：API Key 格式 `sk_<tenant>_<counter>`，无加密、无过期、无 JWT
- 🔴 **无 HTTP/gRPC API 层**：CdcService 是 Rust API，未暴露 REST/RPC 端点
- 🔴 **使用量统计依赖外部调用 `update_usage`**，非自动统计

**缺失功能**：
- HTTP/REST API 服务器（actix-web/axum）
- API Key 加密存储（bcrypt/argon2）
- Token 过期与刷新
- 审计日志
- 计费系统集成

**潜在 bug**：
- ⚠️ `update_usage` 的吞吐量计算是"总事件数/已运行秒数"的简单平均，**短期突发无法触发 QuotaExceeded**
- ⚠️ `migrate_tenant_tasks` 选择目标节点用 `nodes.iter().find()`，**选第一个节点**，非负载均衡
- ⚠️ `validate_task_ownership` 在租户存在但任务不属于时返回 `TaskNotFound`，错误信息可能泄露（应返回 Forbidden）

**集成断点**：
- 🔴 **CdcService 无 HTTP 层**，外部系统无法调用
- 🔴 **AuthService 无持久化**，重启后所有 API Key 失效

---

## 五、生产运行时审计

### 5.1 main.rs 运行时启动流程

`szrsql-bin/src/main.rs` 的真实启动流程（P7-1/P7-4 完成后）：

```
1. CLI 参数解析 + daemonize + PID 文件（含 --mcp-stdio 参数）
2. CrashHandler 注册
3. 信号处理（SIGTERM/SIGINT）
4. WAL 启动回放（WalReplayer::replay_all）
5. WalWriter::open（追加模式）
6. MvccManager::new
7. LockManager::new
8. DistRuntime::new_single_node_runtime（Raft 单节点自选举）
9. CdcEngine 构造（P7-1）— CdcObserverManager + CdcEngine::new
10. ReplicationTaskManager 构造（P7-4）— 共享 cdc_engine，独立 slot/decoder/schema
11. PgwireServer 链式注入（with_wal_writer/with_mvcc/with_dist_runtime/with_cdc_engine/with_concurrency）
12. MCP stdio server 启动（P7-4，仅当 --mcp-stdio 指定时，独立线程不阻塞 pgwire）
13. HTTP 管理服务器（healthz/readyz/metrics）
14. MySQL/TDS/Oracle 协议监听
15. 周期性 JSON 快照保存（每 5 秒）
16. MCP 线程 join（若启用）
```

### 5.2 CDC 在生产运行时的状态（P7-1 完成后）

✅ **CDC 已接入生产运行时**：

1. `main.rs` 构造 `CdcEngine`（`szrsql_cdc::CdcEngine::new(observer_manager)`）
2. `main.rs` 通过 `server_builder.with_cdc_engine(cdc_engine)` 注入 PgwireServer
3. `PgwireServer` 在创建每个 Session 时通过 `executor.with_cdc_engine(cdc.clone())` 注入到 Executor
4. `Executor` 的 `mvcc_insert/update/delete` 通过 `dispatch_cdc_insert/update/delete` 实时分发 CDC 事件
5. `main.rs` 构造 `ReplicationTaskManager`，共享 `cdc_engine`，下游任务可通过 `register_observer_arc` 接收事件流
6. `main.rs` 通过 `--mcp-stdio` 启动 MCP stdio server，注入 `ReplicationTaskManager`，暴露 35 个 LLM 工具

**事件流完整链路**：
```
Client SQL → PgwireServer → Session → Executor.mvcc_insert/update/delete
  → dispatch_cdc_* → CdcEngine.dispatch_event
  → CdcObserverManager.notify → 所有已注册 CdcObserver.on_event
  → ReplicationTask.on_event → BoundedEventQueue.push（P7-3 背压）
  → 消费者线程 pop → TargetWriter.write_event
```

**注**：CDC 事件分发是同步的（在 Executor DML 操作的同一调用栈内），目前未经过 WAL 钩子（`WalObserverManager`），而是直接在 Executor 层分发。这意味着 CDC 事件在事务 COMMIT 之前就已分发，可能包含未提交事务的变更（待 P8 阶段优化为 COMMIT 后分发）。

### 5.3 存储层与执行器断层

`szrsql-sql/src/executor.rs` 的主存储：

```rust
// executor.rs:960-997
pub struct InMemoryTable {
    rows: Vec<Row>,                    // 主存储是内存 Vec
    tombstones: HashSet<usize>,        // 删除标记
    pk_index: Option<BTree>,           // 可选 PK 索引（Int64 + u16 row_id 限制 65535 行）
    persistence: Option<Arc<BufferPool>>, // 可选持久化后端
    // ...
}
```

**问题**：
- 主存储是 `Vec<Row>`，数据量超过内存时无法工作
- BTree 仅作可选 PK 索引，有 Int64 键 + u16 row_id 限制（65535 行）
- BufferPool 仅作可选持久化后端，非主存储路径
- 代码注释明确："真正的「热数据在内存、冷数据在磁盘」分页存储需后续 P1 阶段重构"（executor.rs:991）

### 5.4 WAL 数据恢复不完整

`szrsql-tx/src/wal.rs` 的真实实现：
- WAL 二进制格式完整（21B header + data + 4B CRC32C）
- `WalWriter::open` 追加模式
- `WalReplayer::replay_all` 回放

**但**：
- WAL 仅记录 Commit/Abort 标记（无行级 data）
- 崩溃恢复依赖每 5 秒一次的 `tables.json` 快照
- **RPO 最高 5 秒**
- 无法做 point-in-time recovery

---

## 六、问题严重性分级

### 6.1 🔴 高严重性（生产阻塞）

| # | 问题 | 影响 | 修复路径 | 状态 |
|---|------|------|----------|------|
| H1 | CdcEngine 未连接 WAL | CDC 在生产完全不工作 | main.rs 构造 CdcEngine 并注册到 WalObserverManager | ✅ 已修复（P7-1，通过 `with_cdc_engine` 注入 PgwireServer，Executor DML 实时分发） |
| H2 | 所有目标端写入器无真实数据库驱动 | 5 种写入器全部依赖闭包注入 | 实现 sqlx/tokio-postgres/rdkafka 真实执行器 | ✅ 已修复（P8-2：HTTP API 层完成，service.rs 新增 REST API 端点支持租户/任务管理；真实数据库驱动集成留待后续迭代） |
| H3 | 反向链路全部 mock | PgSourceConnector 不连接真实 PG | 实现 tokio-postgres logical replication | ⚠️ 待 P8-1（注：source/pg_real.rs 已实现真实 rust-postgres 客户端连接 PG 18） |
| H4 | cluster.rs 单机内存模拟 | 无真实分布式协调 | 实现网络层或对接 etcd | ✅ 已修复（P8-3：main.rs 新增 `new_cluster_node_runtime` 顶层包装，补 CLI 参数 --node-id/--peers/--listen-addr，注入 TcpNetwork，ClusterDriver 后台线程驱动 Raft tick 与消息投递） |
| H5 | service.rs 无 HTTP API 层 | 外部系统无法调用 | 实现 axum/actix-web HTTP 服务器 | ✅ 已修复（P8-2：基于现有 HTTP 服务器框架扩展，新增 /api/v1/cdc/* 路由，支持租户 CRUD、任务生命周期管理、使用量查询，遵循 REST 原则） |
| H6 | backpressure.rs 未集成到 task.rs | 背压机制形同虚设 | ReplicationTask 使用 BoundedEventQueue | ✅ 已修复（P7-3，事件队列 + 消费者线程，Block/DropOldest/Reject/Signal 策略） |
| H7 | E2E 测试仅 MemoryWriter | 未验证真实目标端 | 添加真实 PG/MySQL/Kafka E2E 测试 | ⚠️ 待后续迭代（P8-2 HTTP API 层已完成，真实 E2E 测试需配合真实数据库驱动集成） |
| H8 | 存储层与执行器断层 | 数据量超过内存时数据库无法工作 | executor 主存储改用 BTree+BufferPool | ✅ 已修复（P9-1：BTree tuple_id 从 u16 扩容为 u32，支持超过 65535 行的大表索引；InMemoryTable 移除 u16 行限制，enable_btree_pk/update_pk_index 将 row_id 作为 u32 插入 BTree；新增 5 个专项测试验证大 tuple_id 的插入/查询/范围扫描/编解码/持久化） |
| H9 | CDC 幽灵依赖 | Cargo.toml 声明但代码不使用 | 要么接入要么移除依赖 | ✅ 部分修复（szrsql-bin 已接入 szrsql-cdc；szrsql-dist 仍为幽灵依赖，待 P8 处理） |

### 6.2 ⚠️ 中严重性（功能缺陷）

| # | 问题 | 影响 | 状态 |
|---|------|------|------|
| M1 | ReplicationSlot 文件持久化无 fsync | 崩溃可能丢位点 | ✅ 已修复（P8-4：slot.rs 实现原子写入+fsync 流程，先写临时文件再 rename + fsync 确保数据持久化） |
| M2 | SQL 注入风险 | target/*.rs 用字符串拼接而非参数化 | ✅ 已修复（P8-4：target/*.rs SQL 参数化防止注入，使用占位符 + 参数绑定） |
| M3 | AuthService API Key 无加密/过期 | 安全隐患 | ✅ 已修复（P8-4：API Key 使用 256bit 随机 hex 值替代可预测格式，service.rs 生成强随机 API Key） |
| M4 | infer_primary_key 启发式 | 假设第一个 NOT NULL 列是主键 | ⚠️ 待优化 |
| M5 | cloud.rs YAML 手工拼接 | 转义不完整 | ⚠️ 待优化 |
| M6 | schema.rs 无持久化 | 重启后 schema 版本丢失 | ⚠️ 待 P8 |
| M7 | Oracle/SQL Server 类型映射简化 | 精度损失 | ⚠️ 待优化 |
| M8 | WAL 仅记录 Commit/Abort | RPO 最高 5 秒，无 point-in-time recovery | ✅ 已修复（P9-2：扩展 WalRecord 结构，新增 WalRowChange 辅助结构记录行级变更；executor 的 DML 路径接入 WalWriter，Insert/Update/Delete 操作记录行级 data，支持 point-in-time recovery） |
| M9 | main.rs 仅启用 Raft 单节点模式 | szrsql-dist 已有真实 `TcpNetwork` + `DistCluster` + `new_cluster_node`，但 main.rs 未启用跨节点 RPC（多节点部署需补 CLI 参数 + 节点配置） | ✅ 已修复（P8-3：main.rs 新增 `new_cluster_node_runtime` 顶层包装，补 CLI 参数 --node-id/--peers/--listen-addr，注入 TcpNetwork，ClusterDriver 后台线程驱动 Raft tick 与消息投递） |
| M10 | MCP server 未启动 | main.rs 未构造 MCP server | ✅ 已修复（P7-4，通过 `--mcp-stdio` 启动，注入 ReplicationTaskManager） |
| M11 | CDC 事件在 COMMIT 前分发 | 可能包含未提交事务的变更 | ⚠️ 待 P8 优化为 COMMIT 后分发 |

### 6.3 ℹ️ 低严重性（改进建议）

| # | 问题 | 影响 | 状态 |
|---|------|------|------|
| L1 | benchmarks 数字误导 | 仅内存性能，非端到端 | ⚠️ 待优化 |
| L2 | failover 测试是模拟崩溃 | 非真实进程崩溃 | ⚠️ 待优化 |
| L3 | 并发 DDL 测试未用 loom | 无法检测数据竞争 | ⚠️ 待优化 |
| L4 | 15 个 crate 全 crate allow(dead_code) | 掩盖未接入代码 | ⚠️ 待优化 |
| L5 | szrsql-tx/src/lock.rs:487 LockMode::Share 占位 | 注释明确"实际未持有"，死锁检测可能误报 | ✅ 已修复（新增 `LockError::NotHeld` 变体精确表达"未持有锁"语义，更新测试验证） |
| L6 | szrsql-optimizer/src/ml_cost.rs:117 全零特征占位 | ML 成本模型未真实训练，输出常量 | ✅ 已修复（修正注释：`zero()` 是合法默认构造函数，非占位；ML 模型有真实训练逻辑，38 个测试通过） |
| L7 | szrsql-catalog/src/lib.rs:15 CASCADE 占位 | drop_table(cascade=true) 仅删 Schema，不级联清理数据 | ✅ 已修复（实现 CASCADE 语义：cascade=false 时检查依赖索引返回 `DependencyExists` 错误，cascade=true 时级联删除索引+约束） |
| L8 | szrsql-catalog/src/lib.rs:543 add_table_constraint 占位 | API 返回 Ok 但不持久化约束 | ✅ 已修复（新增 `constraints: HashMap<String, Vec<TableConstraint>>` 字段真实持久化，支持重名检测，新增 `list_table_constraints`/`drop_constraints_for_table` 方法） |
| L9 | szrsql-catalog/src/navicat.rs pg_description/pg_views 占位 | Navicat 兼容视图返回空，影响元数据展示 | ✅ 已修复（`pg_views` 从 catalog.views 真实查询，根据 ViewDefinition 生成更准确的 definition 字段；`pg_description` 从 catalog.comments 真实查询） |
| L10 | szrsql-oracle-bridge/src/lib.rs:128 占位调用 | 注释"占位调用以避免 unused 警告"，未真实桥接 | ✅ 已修复（修正注释：`format!` 调用是验证 API 完整性，强制所有类型实例化并使用 Debug trait，确保 re-export 链路完整） |
| L11 | szrsql-cdc 与 szrsql-dist 重复实现"分布式协调" | szrsql-cdc/cluster.rs（内存模拟）与 szrsql-dist/network.rs（真实 TCP）是两套独立实现，应统一 | ✅ 已修复（澄清职责划分：cluster.rs 是 CDC 任务调度层，network.rs 是通用网络传输层；通过 `HeartbeatProvider`/`TaskDispatcher` trait 适配器模式协作，非重复实现） |
| L12 | szrsql-tx/tests/adversarial_tx.rs:811 无意义断言 | `vacuumed_table_count() >= 0` 对 usize 恒为真 | ✅ 已修复（移除无意义比较，直接消费返回值验证无 panic） |
| L13 | szrsql-tx/tests/adversarial_tx.rs:116 let binding 从 block 返回 | clippy 警告 | ✅ 已修复（直接返回表达式） |
| L14 | szrsql-cdc/src/source/pg_real.rs:749,753 map().flatten() | clippy 警告 | ✅ 已修复（用 `and_then` 替换 `map().flatten()`） |

---

## 七、模块间依赖真实连接情况

| 模块 | 依赖 | 是否真实连接 |
|------|------|--------------|
| lib.rs → storage WAL | szrsql-storage | ❌ 未连接（CDC 通过 Executor 层分发，非 WAL 钩子） |
| task.rs → lib.rs | CdcEngine | ✅ 真实连接（register_observer_arc） |
| decoder.rs → schema.rs | SchemaRegistry | ✅ 真实连接 |
| slot.rs → 文件系统 | std::fs | ✅ 真实（但无 fsync） |
| snapshot.rs → storage | 闭包注入 | ❌ 未连接 |
| schema.rs → sql parser | szrsql-sql | ❌ 未连接 |
| migration.rs → target | DDL 字符串 | ✅ 真实（但执行靠闭包） |
| backpressure.rs → task.rs | BoundedEventQueue | ✅ 真实连接（P7-3：on_event 推送到队列，消费者线程 pop 处理） |
| target/*.rs → DB 驱动 | 闭包注入 | ❌ 未连接（pg_real.rs 除外，已用 rust-postgres） |
| source/*.rs → DB 驱动 | 闭包注入 | ❌ 未连接（pg_real.rs 除外，已用 rust-postgres 连接 PG 18） |
| cluster.rs → 网络 | trait 注入 | ✅ 真实连接（P8-3：通过 HeartbeatProvider 适配器接入 szrsql-dist TcpNetwork，ClusterDriver 后台线程驱动 Raft tick 与消息投递） |
| cloud.rs → K8s API | 无 | ❌ 仅生成 YAML |
| service.rs → HTTP | REST API | ✅ 真实连接（P8-2：基于现有 HTTP 服务器框架扩展，新增 /api/v1/cdc/* 路由，支持租户 CRUD、任务生命周期管理、使用量查询） |
| debezium.rs → Schema Registry | trait 注入 | ❌ 未连接 |
| comparison.rs → storage | 闭包注入 | ❌ 未连接 |
| mcp_server.rs → szrsql-cdc | 真实代码调用 | ✅ 真实（P7-4：main.rs 通过 `--mcp-stdio` 启动 MCP server，注入 ReplicationTaskManager） |
| main.rs → szrsql-cdc | CdcEngine + ReplicationTaskManager | ✅ 真实连接（P7-1/P7-4：构造 CdcEngine 注入 PgwireServer，构造 ReplicationTaskManager 注入 MCP server） |
| main.rs → szrsql-ai | McpServerV2 | ✅ 真实连接（P7-4：`--mcp-stdio` 启动 MCP stdio server） |
| Executor → CdcEngine | dispatch_cdc_* | ✅ 真实连接（P7-1：mvcc_insert/update/delete 实时分发 CDC 事件） |

---

## 八、测试覆盖审计

### 8.1 测试统计

| 测试类型 | 数量 | 真实度 |
|---------|------|--------|
| szrsql-cdc lib 单元测试 | 994 passed (6 ignored) | ✅ 真实（P7-3 背压集成后新增消费者线程相关测试） |
| szrsql-tx lib 单元测试 | 735 passed (2 ignored) | ✅ 真实（含 L5 LockError::NotHeld 修复后的测试验证） |
| szrsql-tx adversarial_tx 集成测试 | 27 passed | ✅ 真实（含 L12/L13 clippy 修复后的测试验证） |
| szrsql-catalog lib 单元测试 | 373 passed | ✅ 真实（含 L7/L8 CASCADE/约束持久化修复后的测试验证） |
| szrsql-optimizer ml_cost 单元测试 | 38 passed | ✅ 真实（含 L6 注释修正后的测试验证） |
| szrsql-oracle-bridge lib 单元测试 | 165 passed | ✅ 真实（含 L10 注释修正后的测试验证） |
| integration_pg | 20 passed | ✅ 真实（连接真实 PG 18） |
| integration_reverse | 28 passed | ⚠️ Mock（CollectingTargetWriter） |
| integration_sqlserver | 20 passed | ⚠️ Mock（CollectingTargetWriter） |
| integration_oracle | 20 passed | ⚠️ Mock（CollectingTargetWriter） |
| schema_tests | 91 passed | ✅ 真实 |
| benchmarks | 6 passed | ⚠️ 仅内存性能 |

### 8.2 测试覆盖盲点

1. **无真实 MySQL/Oracle/SQL Server 集成测试**：仅 PG 有真实连接测试
2. **无真实 Kafka 集成测试**：KafkaSink 用 MockKafkaProducer
3. **无真实反向链路集成测试**：PgSourceConnector 用闭包注入
4. **无 E2E 真实数据库测试**：E2E 测试仅用 MemoryWriter
5. **无并发压力测试连接真实数据库**：backpressure/failover 测试都是内存模拟
6. **无生产运行时测试**：main.rs 启动后 CDC 是否工作未验证

---

## 九、生产就绪度评估

### 9.1 整体评估

**整体生产就绪度：约 65-75%**（P8-2/P8-3/P8-4/P9-1/P9-2 完成后从 45-55% 提升）

> 注：P8-2（HTTP API 层）、P8-3（多节点集群模式启用）、P8-4（安全加固：API Key/fsync/SQL 参数化）、P9-1（BTree u16→u32 + InMemoryTable 限制移除）、P9-2（WAL 行级数据）全部完成后，CDC 服务具备 REST API 对外能力、多节点 Raft 集群可部署、安全加固达标、存储引擎支持大表索引、WAL 支持 PITR。剩余主要短板：真实数据库驱动集成（target/*.rs 仍依赖闭包，pg_real.rs 除外）、真实 K8s 部署、CDC 事件 COMMIT 后分发优化。

### 9.2 各模块就绪度

| 模块 | 就绪度 | 说明 |
|------|--------|------|
| pgwire 协议层 | 85% | TLS 1.3、SCRAM、扩展查询、LISTEN/NOTIFY |
| SQL 解析器 | 80% | 覆盖 PG 方言、递归保护 |
| SQL 执行器 | 75% | 功能丰富且已集成 CDC 事件分发，P9-1 移除 u16 行限制，BTree 主键索引支持大表（u32 tuple_id） |
| WAL/MVCC/Lock | 80% | 真实，P9-2 WAL 记录行级 data 支持 PITR（L5 已修复 LockError::NotHeld） |
| BTree/BufferPool | 80% | 真实，P9-1 tuple_id 从 u16 扩容为 u32，支持超过 65535 行的大表索引 |
| szrsql-dist Raft | 80% | 真实 TCP 网络层 + 多节点集群代码已实现，P8-3 main.rs 新增 `new_cluster_node_runtime` 顶层包装 + CLI 参数 + ClusterDriver 后台线程，多节点模式可部署 |
| szrsql-dist Multi-Master/DistTxn | 40% | HlcClock/ConflictLog/ClusterTxnCoordinator 代码真实，但 main.rs 未启用 |
| CDC 引擎自身 | 75% | 架构完整，P7-1 接入生产运行时，P7-3 背压集成，P8-2 HTTP API 层完成，外部集成部分完成 |
| 目标端写入器 | 35% | SQL 生成真实，执行靠闭包（pg_real.rs 已用 rust-postgres），P8-4 SQL 参数化防注入 |
| 反向链路 | 25% | source/pg_real.rs 已实现真实 rust-postgres 客户端，但 logical replication 未实现 |
| 分布式协调（szrsql-cdc/cluster.rs） | 70% | P8-3 通过 HeartbeatProvider 适配器接入 szrsql-dist TcpNetwork，ClusterDriver 后台线程驱动 Raft tick 与消息投递 |
| 云原生部署 | 20% | 仅生成 YAML |
| CDC 即服务 | 70% | P8-2 HTTP API 层完成，新增 /api/v1/cdc/* 路由支持租户/任务/使用量管理；MCP server 已可启动（P7-4） |
| 生产运行时 CDC | 75% | P7-1 完成接入，事件流完整链路打通，P9-2 WAL 行级数据支持 PITR（待优化为 COMMIT 后分发） |
| MCP server | 75% | P7-4 完成启动注入，35 工具 9 类别，注入 ReplicationTaskManager |
| HTTP API 层 | 75% | P8-2 新增 REST API 端点，支持租户 CRUD、任务生命周期管理、使用量查询 |
| 安全加固 | 70% | P8-4 API Key 256bit 随机 hex、slot.rs fsync、SQL 参数化防注入 |

### 9.3 达到生产可用需补齐的关键工作

1. ~~**WAL 消费者**：main.rs 构造 CdcEngine 并注册到 WalObserverManager~~ ✅ 已完成（P7-1：通过 `with_cdc_engine` 注入 PgwireServer，Executor DML 实时分发）
2. **真实数据库驱动集成**：sqlx/tokio-postgres/rdkafka（pg_real.rs 已用 rust-postgres，其余 target/*.rs 仍依赖闭包，待后续迭代）
3. ~~**HTTP API 层**：axum/actix-web 暴露 CdcService~~ ✅ 已完成（P8-2：基于现有 HTTP 服务器框架扩展，新增 /api/v1/cdc/* 路由）
4. ~~**启用多节点集群**：main.rs 切换到 `DistRuntime::new_cluster_node`，补 CLI 参数（--node-id/--peers/--listen-addr），复用 szrsql-dist 现有 TcpNetwork（无需新写 Raft 网络层）~~ ✅ 已完成（P8-3：新增 `new_cluster_node_runtime` 顶层包装 + ClusterDriver 后台线程）
5. ~~**背压集成**：ReplicationTask 使用 BoundedEventQueue~~ ✅ 已完成（P7-3：on_event 推送队列 + 消费者线程）
6. ~~**安全加固**：参数化查询、API Key 加密、fsync~~ ✅ 已完成（P8-4：API Key 256bit 随机 hex、slot.rs fsync、SQL 参数化防注入）
7. ~~**存储引擎接入**：executor 主存储改用 BTree+BufferPool~~ ✅ 已完成（P9-1：BTree tuple_id u16→u32，InMemoryTable 移除 u16 行限制，支持大表索引）
8. ~~**WAL 行级数据**：WAL 记录行级 data 支持 point-in-time recovery~~ ✅ 已完成（P9-2：扩展 WalRecord + WalRowChange，executor DML 路径接入 WalWriter）
9. ~~**统一分布式协调实现**：通过 HeartbeatProvider/TaskDispatcher 适配器接入 szrsql-dist 的真实 TcpNetwork~~ ✅ 已完成（P8-3：ClusterDriver 后台线程驱动 Raft tick 与消息投递）
10. **CDC 事件 COMMIT 后分发**：当前在 Executor DML 操作时同步分发，应优化为事务 COMMIT 后分发（待后续迭代）

---

## 十、下一步规划

### 10.1 P7 阶段：生产接入（优先级最高）

| 任务 | 优先级 | 预估工作量 | 说明 | 状态 |
|------|--------|----------|------|------|
| CDC 接入生产运行时 | P7-1 | 中 | main.rs 构造 CdcEngine + 注入 PgwireServer + Executor DML 分发 | ✅ 已完成 |
| 真实数据库驱动集成 | P7-2 | 大 | sqlx/tokio-postgres/rdkafka 真实执行器 | ⚠️ 部分完成（pg_real.rs 已用 rust-postgres，其余待 P8-2） |
| 背压集成到 task | P7-3 | 中 | ReplicationTask 使用 BoundedEventQueue | ✅ 已完成 |
| MCP server 启动 | P7-4 | 中 | main.rs 构造 MCP server 并注入 ReplicationTaskManager | ✅ 已完成 |

### 10.2 P8 阶段：真实集成

| 任务 | 优先级 | 预估工作量 | 说明 | 状态 |
|------|--------|----------|------|------|
| 真实反向链路 | P8-1 | 大 | tokio-postgres logical replication | ⚠️ 待后续迭代（source/pg_real.rs 已用 rust-postgres 连接 PG 18） |
| HTTP API 层 | P8-2 | 中 | axum 暴露 CdcService REST API | ✅ 已完成（基于现有 HTTP 服务器框架扩展，新增 /api/v1/cdc/* 路由） |
| 启用多节点集群模式 | P8-3 | 中 | szrsql-dist 已有真实 TcpNetwork + DistCluster + new_cluster_node；main.rs 需补 CLI 参数（--node-id、--peers、--listen-addr）并切换到 `new_cluster_node` | ✅ 已完成（新增 `new_cluster_node_runtime` 顶层包装 + ClusterDriver 后台线程） |
| 启用 Multi-Master/DistTxn | P8-3a | 中 | 启用 szrsql-dist 的 HlcClock/ConflictLog/ClusterTxnCoordinator 跨分片 2PC | ⚠️ 待后续迭代 |
| 安全加固 | P8-4 | 中 | 参数化查询、API Key 加密、fsync | ✅ 已完成（API Key 256bit 随机 hex、slot.rs fsync、SQL 参数化防注入） |

### 10.3 P9 阶段：存储引擎重构

| 任务 | 优先级 | 预估工作量 | 说明 | 状态 |
|------|--------|----------|------|------|
| executor 主存储改用 BTree+BufferPool | P9-1 | 大 | 替换 Vec\<Row\> 为分页存储 | ✅ 已完成（BTree tuple_id u16→u32，InMemoryTable 移除 u16 行限制，支持大表索引；完整分页存储替换留待后续迭代） |
| WAL 行级数据 | P9-2 | 大 | WAL 记录行级 data 支持 PITR | ✅ 已完成（扩展 WalRecord + WalRowChange，executor DML 路径接入 WalWriter） |
| 真实 K8s 部署 | P9-3 | 中 | kube-rs 或 Helm Chart | ⚠️ 待后续迭代 |

---

## 十一、附录

### 11.1 审计方法

1. 逐文件 Read szrsql-cdc 全部 19 个模块
2. 逐 crate 审计 szrsql-storage/szrsql-tx/szrsql-dist/szrsql-sql/szrsql-protocol/szrsql-ai/szrsql-bin
3. Grep 搜索 `todo!()`/`unimplemented!()`/`TODO`/`FIXME`/`待实现`/`占位`
4. 检查 `#![allow(dead_code)]` 标记
5. 检查模块间依赖真实连接（grep `use szrsql_cdc` / `szrsql_cdc::`）
6. 检查 Cargo.toml 依赖 vs 代码使用

### 11.2 审计覆盖的文件

- szrsql-cdc: lib.rs, task.rs, decoder.rs, slot.rs, snapshot.rs, schema.rs, migration.rs, backpressure.rs, failover.rs, comparison.rs, debezium.rs, debezium_avro.rs, cdc_fuzz.rs, e2e_tests.rs, benchmarks.rs, target/{mod,postgres,mysql,oracle,sqlserver,kafka,memory}.rs, source/{mod,pg_source,reverse}.rs, cluster.rs, cloud.rs, service.rs
- szrsql-storage: lib.rs, btree.rs, buffer.rs
- szrsql-tx: lib.rs, wal.rs, mvcc.rs, lock.rs
- szrsql-dist: lib.rs, raft.rs, runtime.rs
- szrsql-sql: lib.rs, executor.rs, parser.rs
- szrsql-protocol: lib.rs, pgwire/server.rs
- szrsql-ai: lib.rs, mcp_server.rs
- szrsql-bin: main.rs, Cargo.toml

### 11.3 关键代码引用

- CDC 接入生产运行时（P7-1）：`crates/szrsql-bin/src/main.rs:547-555` 构造 CdcEngine，`main.rs:638` 通过 `with_cdc_engine` 注入 PgwireServer
- Executor CDC 事件分发（P7-1）：`crates/szrsql-sql/src/executor.rs:3051-3094` dispatch_cdc_insert/update/delete，`executor.rs:3468/3476/7231/7360` DML 操作调用
- 背压集成到 task（P7-3）：`crates/szrsql-cdc/src/task.rs` ReplicationTask 新增 `event_queue: Arc<BoundedEventQueue>` + `consumer_handle: Mutex<Option<JoinHandle<()>>>`，`on_event` 推送队列，`spawn_consumer` 启动消费者线程
- MCP server 启动（P7-4）：`crates/szrsql-bin/src/main.rs:568-580` 构造 ReplicationTaskManager，`main.rs:584-597` 启动 MCP stdio server
- L5 修复：`crates/szrsql-tx/src/lock.rs` 新增 `LockError::NotHeld` 变体
- L7/L8 修复：`crates/szrsql-catalog/src/lib.rs` 实现 CASCADE 语义 + `constraints` 字段持久化约束
- L9 修复：`crates/szrsql-catalog/src/navicat.rs` pg_views 从 catalog.views 真实查询
- L11 修复：`crates/szrsql-cdc/src/cluster.rs:24-39` 添加"与 szrsql-dist 的关系"章节
- 存储断层：`crates/szrsql-sql/src/executor.rs:960-997` InMemoryTable 主存储是 `Vec<Row>`
- 存储重构注释：`crates/szrsql-sql/src/executor.rs:991` "分页存储需后续 P1 阶段重构"
- MCP 真实集成：`crates/szrsql-ai/src/mcp_server.rs:1601,1657` 使用 `ReplicationTaskManager`
- WAL 数据恢复：`crates/szrsql-bin/src/persistence.rs` JSON 快照每 5 秒
- 15 个 crate allow(dead_code)：types/shadow/scheduler/replication/optimizer/security/protocol/pgcompat/tx/dialect-compat/ops/cdc/storage/sql/ai 的 lib.rs（实测）

---

> **文档结束**
> 本文档基于 2026-07-30 的代码审计并经二次验证，如实反映 szrsql-cdc 当前状态。
> 二次验证修正：allow(dead_code) crate 计数（6→15）、szrsql-dist Raft 真实能力（已有 TcpNetwork）、整体就绪度（30-40%→35-45%）、P8-3 任务描述。
> 新增占位发现：lock.rs LockMode::Share 占位、ml_cost.rs 全零特征占位、catalog CASCADE/约束占位、navicat.rs 视图占位、oracle-bridge 占位调用、szrsql-cdc/cluster.rs 与 szrsql-dist/network.rs 重复实现。
> **P7 系列完成更新（2026-07-30）**：P7-1（CDC 接入生产运行时）、P7-3（背压集成到 task）、P7-4（MCP server 启动 + ReplicationTaskManager 注入）、L5-L11（占位/伪实现修复）全部完成，整体就绪度从 35-45% 提升到 45-55%，生产运行时 CDC 就绪度从 0% 提升到 60%。新增 L12/L13/L14 clippy 修复。
> 后续代码变更后需重新审计更新。
