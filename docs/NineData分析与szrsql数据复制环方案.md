# NineData 社区版分析 & szrsql 内部数据复制闭环方案

> **文档版本**：7.0（基于代码实际能力重新梳理）
> **分析日期**：2026-07-29
> **重写日期**：2026-07-31
> **核心原则**：只报告代码中实际看到的内容，不编造功能，不夸大能力
> **本次重写重点**：v6.0 误标 HTTP API 为"未启用"，实际 main.rs:948-954 已调用 `with_cdc_service`；本次重新逐文件验证所有功能项的真实状态

---

## 〇、阅读指南：状态标记说明

为消除歧义，本文档所有功能项严格使用以下三态标记：

| 标记 | 含义 |
|------|------|
| ✅ **生产已启用** | 代码已实现 **且** main.rs 运行时真实调用 |
| 🟡 **代码已实现但未启用** | 代码存在于 crate 中，但 main.rs 未接入（生产不可用） |
| ❌ **未实现/缺失** | 代码中不存在，或仅有占位/伪实现 |

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
│   ├── szrsql-protocol/   # pgwire/mysql 协议 + HTTP
│   ├── szrsql-cdc/        # CDC 变更数据捕获
│   ├── szrsql-ai/         # MCP server/NL2SQL/RAG
│   └── szrsql-bin/        # 运行时入口
```

### 2.2 各 Crate 真实能力速览（基于代码验证）

| Crate | 代码真实度 | 生产运行时接入 | 与 CDC 集成 |
|-------|--------|--------------|------------|
| szrsql-storage BTree | ✅ 真实（P9-1: tuple_id u32） | ✅ 是 — 作 PK 索引 | ❌ 无 |
| szrsql-storage BufferPool | ✅ 真实 | ✅ 是 — OPT-3 main.rs:633-662 启用单表持久化 | ❌ 无 |
| szrsql-tx WAL | ✅ 真实（P9-2: 行级 data + CRC32C） | ✅ 是 — main.rs:922 注入 wal_writer | ✅ WalObserver trait 已实现 |
| szrsql-tx MVCC | ✅ 真实（SSI、First-Committer-Wins） | ✅ 是 | ❌ 无 |
| szrsql-tx Lock | ✅ 真实（S/X、升级、FIFO、超时） | ✅ 是 | ❌ 无 |
| szrsql-dist Raft | ✅ 真实（TcpNetwork + DistCluster） | ✅ 是 — P8-3 main.rs:691-790 支持 `--cluster-mode cluster` | ❌ 无 |
| szrsql-dist Percolator | ✅ 真实（TSO+2PC+resolve_lock） | 🟡 TSO 已用，跨分片 2PC 未启用 | ❌ 无 |
| szrsql-dist Multi-Master | ✅ 真实（HlcClock、ConflictLog） | ❌ main.rs 未启用 | ❌ 无 |
| szrsql-dist DistTxn | ✅ 真实（ClusterTxnCoordinator） | ❌ main.rs 未启用跨节点协调 | ❌ 无 |
| szrsql-sql parser | ✅ 真实（sqlparser-rs + 递归保护） | ✅ 是 | ❌ 无 |
| szrsql-sql executor | ✅ 真实（火山模型 + 30+ 特性） | ✅ 是 | ✅ dispatch_cdc_* 实时分发 |
| szrsql-protocol pgwire | ✅ 真实（TLS 1.3、SCRAM、扩展查询） | ✅ 是 | ✅ Session.with_cdc_engine 注入 |
| szrsql-protocol http | ✅ 真实（/api/v1/cdc/* 端点已实现） | ✅ 是 — main.rs:948-954 调用 with_cdc_service | ✅ 注入 CdcService |
| szrsql-ai MCP server | ✅ 真实（35 工具，9 类别） | ✅ 是 — `--mcp-stdio` 启动 | ✅ 注入 ReplicationTaskManager |
| szrsql-cdc 引擎 | ✅ 真实（多 target/source） | ✅ 是 — main.rs:831 构造 CdcEngine 注入 PgwireServer | 自身 |
| szrsql-cdc service | ✅ 真实（TenantConfig、API Key 256bit） | ✅ 是 — main.rs:950 构造 CdcService 注入 HttpServer | ✅ 通过 HttpServer 暴露 |
| szrsql-cdc source pg_real | ✅ 真实（postgres::Client） | 🟡 代码可用，main.rs 未直接构造 | 自身 |

### 2.3 关键发现（基于代码验证）

1. **CDC 已接入生产运行时（P7-1 ✅）**：`main.rs:831` 构造 `CdcEngine`，`main.rs:920` 通过 `with_cdc_engine` 注入 PgwireServer，Executor DML 操作（`executor.rs:3094/3129/3150/3172`）实时分发 CDC 事件。
2. **MCP server 已可启动（P7-4 ✅）**：`main.rs:867-877` `--mcp-stdio` 启动 MCP stdio server，注入 ReplicationTaskManager。
3. **背压机制已集成到 task（P7-3 ✅）**：`ReplicationTask` 使用 `BoundedEventQueue`，支持 Block/DropOldest/Reject/Signal 4 种策略。
4. **多节点集群模式已启用（P8-3 ✅）**：`main.rs:691-790` 支持 `--cluster-mode cluster`，调用 `new_cluster_node_runtime` + `TcpNetwork` + `ClusterDriver` 后台线程驱动 Raft tick。
5. **BTree 支持大表索引（P9-1 ✅）**：`btree.rs:153` tuple_ids 从 u16 扩容为 u32，`executor.rs:1227` 移除 u16 行限制。
6. **WAL 行级数据已接入（P9-2 ✅）**：`wal.rs:343` 新增 `WalRowChange` + `WalRecord::new_row_*`，executor DML 路径调用，`main.rs:922` 注入 wal_writer。
7. **HTTP API 已接入运行时（P8-2 ✅ — v6.0 误标，v7.0 修正）**：`main.rs:948-954` 真实调用 `with_cdc_service(cdc_service)`，`http.rs:345/632` 端点路由生效，暴露 `/api/v1/cdc/*` REST API（租户 CRUD、任务生命周期、使用量查询）。
8. **BufferPool 持久化已接入（OPT-3 ✅ — v6.0 漏报，v7.0 修正）**：`main.rs:633-662` 为每张已加载表调用 `enable_persistence({data_dir}/{table_name}.db)`，数据文件落盘。
9. **SQL 参数化未真正实现（P8-4 ❌）**：`target/mysql.rs:134,167,373` 仍使用 `format!` 拼接 SQL + 字符串 escape（`replace('\'', "''")`），**非真正参数化查询**。`target/postgres.rs`、`oracle.rs`、`sqlserver.rs` 同样使用拼接。
10. **target writer 无真实数据库驱动（H2 ❌）**：除 `source/pg_real.rs` 真实使用 `postgres::Client`（line 79,100,116）外，`target/mysql.rs`/`postgres.rs`/`oracle.rs`/`sqlserver.rs`/`kafka.rs` 均依赖 `SqlExecutor` 闭包注入，未集成 sqlx/tokio-postgres/tiberius/rdkafka 等真实驱动。
11. **存储层与执行器部分断层**：executor 主存储仍是 `Vec<Row>` + `HashSet<usize>` tombstone，BTree 仅作可选 PK 索引（Int64 限制）。P9-1 移除了行数限制，OPT-3 接入了 BufferPool 单表持久化，但完整分页存储替换未做。
12. **15 个 crate 全 crate 标记 `#![allow(dead_code)]`**，掩盖了未接入执行链路的代码。

---

## 三、NineData 社区版能力分析

### 3.1 NineData 核心能力对标

| 能力域 | NineData 实现 | szrsql 对标状态 |
|-------|--------------|----------------|
| 数据源连接 | 多种数据库驱动 | 🟡 pg_real.rs 用 rust-postgres；其余 target/*.rs 依赖闭包注入 |
| CDC 实时复制 | WAL/日志解码 | ✅ Executor DML 事件分发 + WalObserver |
| 全量数据初始化 | 快照传输 | ✅ snapshot.rs 实现 |
| 结构迁移 | DDL 同步 | ✅ migration.rs 实现 |
| 数据校验 | 一致性比对 | ✅ comparison.rs 实现（42 单元测试） |
| 反向链路 | 回源同步 | 🟡 source/pg_real.rs 用 rust-postgres，logical replication 未实现 |
| 多租户管理 | 租户隔离 | ✅ service.rs TenantConfig/TenantTier 实现；✅ HTTP API 已启用 |
| 任务调度 | 分布式调度 | ✅ task.rs ReplicationTask 实现 |
| HTTP API | RESTful | ✅ http.rs 端点已实现，main.rs:948-954 已启用 |

### 3.2 NineData 架构启示

（略，详见历史版本）

---

## 四、szrsql-cdc 模块详细审计

### 4.1 CdcEngine 核心（`lib.rs`）— ✅ 真实
- 多 target/source 架构
- CdcObserverManager 分发事件
- 集成测试连接真实 PG 18

### 4.2 任务管理（`task.rs`）— ✅ 真实
- ReplicationTask 状态机（Created→Starting→Running→Stopped/Failed）
- P7-3: BoundedEventQueue 背压（Block/DropOldest/Reject/Signal）

### 4.3 WAL 解码（`decoder.rs`）— ✅ 真实
- PostgreSQL WAL 解码

### 4.4 复制槽（`slot.rs`）— ✅ 真实
- P8-4: 原子写入 + fsync 确保持久化（`slot.rs:349-380`：temp file → sync_all → rename → dir sync_all）

### 4.5 快照传输（`snapshot.rs`）— ✅ 真实
### 4.6 Schema 管理（`schema.rs`）— ✅ 真实
### 4.7 DDL 迁移（`migration.rs`）— ✅ 真实
### 4.8 背压（`backpressure.rs`）— ✅ 真实
### 4.9 故障恢复（`failover.rs`）— ✅ 真实
### 4.10 数据比对（`comparison.rs`）— ✅ 真实（42 单元测试）
### 4.11 Debezium 集成（`debezium.rs` / `debezium_avro.rs`）— ✅ 真实

### 4.12 目标端写入器（`target/`）— 🟡 部分真实
| Writer | SQL 生成 | 真实执行 | 参数化 |
|--------|---------|---------|--------|
| mysql.rs | ✅ | ❌ 闭包注入 | ❌ 字符串拼接 + escape |
| postgres.rs | ✅ | ❌ 闭包注入 | ❌ 字符串拼接 |
| oracle.rs | ✅ | ❌ 闭包注入 | ❌ 字符串拼接 |
| sqlserver.rs | ✅ | ❌ 闭包注入 | ❌ 字符串拼接 |
| kafka.rs | ✅ | ❌ 闭包注入 | N/A |
| pg_real.rs（source 侧） | N/A | ✅ rust-postgres `postgres::Client` | ✅ |

**关键缺失**：除 source/pg_real.rs 外，所有 target writer 均依赖闭包注入执行，未接入真实数据库驱动。SQL 拼接存在注入风险（虽有 escape，但非参数化）。

### 4.13 反向链路（`source/`）— 🟡 部分真实
- source/pg_real.rs: ✅ 使用 `postgres::Client`（line 79,100,116）真实客户端
- logical replication: ❌ 未实现
- SourceConnector trait: ✅ 已定义

### 4.14 分布式协调（`cluster.rs`）— ✅ 真实（P8-3）
- 通过 HeartbeatProvider 适配器接入 szrsql-dist TcpNetwork
- ClusterDriver 后台线程驱动 Raft tick 与消息投递
- main.rs:691-790 真实启用 `--cluster-mode cluster`

### 4.15 云原生部署（`cloud.rs`）— ❌ 仅生成 YAML
- 生成 K8s YAML 文件
- 无真实 kube-rs 集成

### 4.16 CDC 即服务（`service.rs`）— ✅ 代码已实现，运行时已启用（v7.0 修正）
- TenantConfig / TenantTier（Free/Pro/Enterprise）✅
- API Key 256bit 随机 hex（P8-4，`service.rs:32-38`）✅
- **main.rs:950-954 构造 CdcService 并通过 `with_cdc_service` 注入 HttpServer** ✅

---

## 五、生产运行时审计

### 5.1 main.rs 运行时启动流程（基于 `szrsql-bin/src/main.rs` 验证）

| 步骤 | 功能 | 状态 | 代码位置 |
|------|------|------|---------|
| 1 | 参数解析（含 --cluster-mode/--node-id/--peers/--raft-listen-addr/--auth-mode/--auth-file） | ✅ | main.rs:55,235-268 |
| 2 | CredentialStore 加载（--auth-mode=scram） | ✅ | main.rs:401-440 |
| 3 | WAL writer 构造 | ✅ | main.rs:510 |
| 4 | MCP server 启动（--mcp-stdio） | ✅ | main.rs:867-877 |
| 5 | OPT-3 BufferPool 单表持久化启用 | ✅ | main.rs:633-662 |
| 6 | DistRuntime（单节点/集群模式） | ✅ | main.rs:691-790 |
| 7 | CdcEngine 构造 | ✅ | main.rs:831 |
| 8 | PgwireServer 注入 dist_runtime | ✅ | main.rs:917 |
| 9 | PgwireServer 注入 cdc_engine | ✅ | main.rs:920 |
| 10 | PgwireServer 注入 wal_writer | ✅ | main.rs:922 |
| 11 | **HttpServer 注入 cdc_service** | **✅ 已启用（v7.0 修正）** | main.rs:948-954 |

### 5.2 CDC 在生产运行时的状态（P7-1 ✅）
- CdcEngine 构造并注入 PgwireServer ✅
- Executor DML 操作通过 dispatch_cdc_* 实时分发 ✅
- ReplicationTaskManager 接收事件流 ✅

### 5.3 存储层状态（P9-1 部分修复 + OPT-3 接入）
- BTree tuple_id u16→u32 ✅（支持大表索引）
- InMemoryTable 移除 u16 行限制 ✅
- OPT-3 BufferPool 单表持久化 ✅（main.rs:633-662，数据落盘到 `{data_dir}/{table_name}.db`）
- **主存储仍为 Vec\<Row\>** ❌（未替换为分页存储，BufferPool 仅作持久化后端，非主存）

### 5.4 WAL 数据恢复（P9-2 ✅）
- WalRecord 扩展行级 data（WalRowChange）✅
- executor DML 路径接入 WalWriter ✅
- main.rs 注入 wal_writer 到 PgwireServer ✅
- 支持 point-in-time recovery ✅

### 5.5 HTTP API 在生产运行时的状态（P8-2 ✅ — v7.0 修正）
- main.rs:948-954 构造 CdcService 并注入 HttpServer ✅
- http.rs:345/632 端点路由生效 ✅
- 暴露 11 个 REST 端点（租户 CRUD、任务生命周期、使用量查询）✅
- 默认无需鉴权（与 healthz/readyz/metrics 一致）；可通过 `--http-auth-token` 启用 Bearer 鉴权 ✅

---

## 六、问题严重性分级

### 6.1 🔴 高严重性（生产阻塞）

| ID | 问题 | 影响 | 状态 | 修复说明 |
|----|------|------|------|---------|
| H1 | 主存储仍为 Vec\<Row\> | 内存限制大表 | 🟡 部分 | P9-1 移除 u16 限制；OPT-3 接入 BufferPool 持久化；完整分页存储替换未做 |
| H2 | target writer 无真实数据库驱动 | 5 种写入器依赖闭包 | ❌ 未修复 | 需集成 sqlx/tokio-postgres/tiberius/rdkafka |
| H3 | ~~HTTP API 未接入运行时~~ | ~~CdcService 无法对外提供服务~~ | ✅ 已修复 | main.rs:948-954 已调用 with_cdc_service（v7.0 修正） |
| H4 | SQL 拼接非参数化 | 注入风险 | ❌ 未修复 | target/*.rs 需改用参数绑定 |
| H5 | logical replication 未实现 | 反向链路不完整 | ❌ 未修复 | source/ 需实现 PG logical replication |

### 6.2 ⚠️ 中严重性（功能缺陷）

| ID | 问题 | 影响 | 状态 | 修复说明 |
|----|------|------|------|---------|
| M1 | ReplicationSlot fsync | 崩溃丢位点 | ✅ 已修复 | P8-4 slot.rs:349-380 原子写入+fsync |
| M2 | AuthService API Key 弱 | 安全隐患 | ✅ 已修复 | P8-4 256bit 随机 hex |
| M3 | Multi-Master/DistTxn 未启用 | 跨节点 2PC 不可用 | ❌ 未启用 | main.rs 未启用 HlcClock/ConflictLog |
| M4 | 云原生部署仅 YAML | 无真实 K8s 集成 | ❌ 未实现 | 需 kube-rs 或 Helm |

### 6.3 ℹ️ 低严重性（改进建议）
（略，详见历史版本）

---

## 七、模块间依赖真实连接情况

| 依赖关系 | 接入方式 | 状态 |
|---------|---------|------|
| main.rs → CdcEngine | PgwireServer.with_cdc_engine | ✅ 真实连接 |
| main.rs → WalWriter | PgwireServer.with_wal_writer | ✅ 真实连接 |
| main.rs → DistRuntime | PgwireServer.with_dist_runtime | ✅ 真实连接 |
| main.rs → MCP server | --mcp-stdio | ✅ 真实连接 |
| main.rs → HttpServer + CdcService | HttpServer.with_cdc_service | ✅ 真实连接（v7.0 修正） |
| main.rs → BufferPool 持久化 | table.enable_persistence | ✅ 真实连接（OPT-3） |
| main.rs → ClusterDriver | --cluster-mode cluster | ✅ 真实连接 |
| main.rs → CredentialStore | --auth-mode scram | ✅ 真实连接 |
| cluster.rs → TcpNetwork | HeartbeatProvider 适配器 | ✅ 真实连接 |
| Executor → CdcEngine | dispatch_cdc_* | ✅ 真实连接 |
| Executor → WalWriter | DML 路径 new_row_* | ✅ 真实连接 |
| target/*.rs → 真实 DB 驱动 | 闭包注入 | ❌ 未连接（source/pg_real.rs 除外） |

---

## 八、测试覆盖审计

### 8.1 测试统计（本次验证）
- szrsql-storage: 1009 passed
- szrsql-sql adversarial: 44 passed
- szrsql-tx P9-2 新增: 9 passed
- szrsql-cdc: 集成测试连接真实 PG 18

### 8.2 已知测试问题（预存，非 P8/P9 引入）
- `sql_compare::diff_test_dml_sequence_1000` 差分比对失败（szrsql vs PG 18 语义差异）
- `lock_fuzz` 并发压力测试在 Windows 下进程崩溃

### 8.3 测试覆盖盲点
- target/*.rs 真实数据库写入无 E2E 测试（依赖闭包）
- 多节点集群模式无跨进程集成测试
- HTTP API 端点已有单元测试（http.rs 内 `route_request` 测试），但无 main.rs 端到端集成测试

---

## 九、生产就绪度评估

### 9.1 整体评估

**整体生产就绪度：约 65-72%**

> 注：v6.0 基线 55-65%。v7.0 上调原因：HTTP API 已接入运行时（H3 修复 ✅）、BufferPool 持久化已接入（OPT-3 ✅）。剩余主要短板：target writer 无真实驱动（H2）、SQL 非参数化（H4）、主存储未分页（H1）、logical replication 未实现（H5）。

### 9.2 各模块就绪度

| 模块 | 就绪度 | 说明 |
|------|--------|------|
| SQL 执行器 | 75% | 功能丰富，已集成 CDC 事件分发，P9-1 移除 u16 限制 |
| WAL/MVCC/Lock | 85% | 真实，P9-2 WAL 行级 data 支持 PITR |
| BTree | 85% | 真实，P9-1 tuple_id u32 支持大表 |
| BufferPool | 80% | 代码真实，OPT-3 已接入单表持久化；主存储未替换为分页 |
| szrsql-dist Raft | 85% | 真实 TCP，P8-3 多节点模式可部署 |
| szrsql-dist Multi-Master/DistTxn | 40% | 代码真实，main.rs 未启用 |
| CDC 引擎 | 80% | 架构完整，P7-1 接入运行时 |
| 目标端写入器 | 30% | SQL 生成真实，执行靠闭包，非参数化 |
| 反向链路 | 25% | pg_real.rs 真实，logical replication 未实现 |
| HTTP API 层 | 85% | 端点已实现，main.rs:948-954 已启用（v7.0 修正） |
| 安全加固 | 60% | API Key/fsync ✅，SQL 参数化 ❌ |
| 云原生部署 | 20% | 仅生成 YAML |
| MCP server | 75% | 35 工具 9 类别，注入 ReplicationTaskManager |
| 生产运行时 CDC | 80% | 事件流完整链路打通，WAL 支持 PITR，HTTP API 可用 |

### 9.3 达到生产可用需补齐的关键工作

| 序号 | 工作 | 优先级 | 状态 |
|------|------|--------|------|
| 1 | 真实数据库驱动集成（sqlx/tokio-postgres/tiberius/rdkafka） | P0 | ❌ 未完成 |
| 2 | **SQL 参数化**（target/*.rs 改用参数绑定） | P0 | ❌ 未完成 |
| 3 | 主存储替换为分页存储（BTree+BufferPool） | P1 | 🟡 部分（P9-1 扩容 tuple_id + OPT-3 接入持久化） |
| 4 | logical replication 实现（反向链路） | P1 | ❌ 未完成 |
| 5 | 启用 Multi-Master/DistTxn | P2 | ❌ 未完成 |
| 6 | 真实 K8s 部署 | P2 | ❌ 未完成 |
| 7 | CDC 事件 COMMIT 后分发（当前同步分发） | P2 | ❌ 未完成 |
| 8 | ~~HTTP API 接入运行时~~ | ✅ | P8-2 已完成（v7.0 修正） |
| 9 | ~~多节点集群模式启用~~ | ✅ | P8-3 已完成 |
| 10 | ~~BTree tuple_id 扩容~~ | ✅ | P9-1 已完成 |
| 11 | ~~WAL 行级数据~~ | ✅ | P9-2 已完成 |
| 12 | ~~API Key 加固~~ | ✅ | P8-4 已完成 |
| 13 | ~~slot fsync~~ | ✅ | P8-4 已完成 |
| 14 | ~~BufferPool 持久化接入~~ | ✅ | OPT-3 已完成 |

---

## 十、下一步规划

### 10.1 已完成阶段

#### P7 阶段：生产接入 ✅（2026-07-30 完成）
- P7-1: CDC 接入生产运行时 ✅
- P7-3: 背压集成到 task ✅
- P7-4: MCP server 启动 + ReplicationTaskManager 注入 ✅
- L5-L11: 占位/伪实现修复 ✅

#### P8 阶段：真实集成（部分完成）
- P8-2: HTTP API 层 ✅（端点代码已实现，main.rs:948-954 已启用 — v7.0 修正）
- P8-3: 多节点集群模式 ✅（main.rs:691-790 真实启用）
- P8-4: 安全加固 🟡（API Key/fsync ✅，SQL 参数化 ❌）

#### P9 阶段：存储引擎重构（部分完成）
- P9-1: BTree u16→u32 + InMemoryTable 限制移除 ✅
- P9-2: WAL 行级数据 ✅

#### OPT 阶段：性能与存储优化
- OPT-3: BufferPool 单表持久化接入 ✅（main.rs:633-662）
- OPT-4: CredentialStore / SCRAM 认证接入 ✅（main.rs:401-440）

### 10.2 待完成关键工作（按优先级）

| 任务 | 优先级 | 说明 |
|------|--------|------|
| **真实数据库驱动集成** | P0 | mysql/postgres/oracle/sqlserver/kafka writer 接入真实驱动（sqlx/tiberius/rdkafka） |
| **target/*.rs SQL 参数化** | P0 | 改用参数绑定，消除字符串拼接注入风险 |
| **主存储分页替换** | P1 | executor 主存储从 Vec\<Row\> 替换为 BTree+BufferPool |
| **logical replication** | P1 | source/ 实现 PG logical replication 协议 |
| **启用 Multi-Master/DistTxn** | P2 | main.rs 启用 HlcClock/ConflictLog/ClusterTxnCoordinator |
| **真实 K8s 部署** | P2 | kube-rs 或 Helm Chart |
| **CDC 事件 COMMIT 后分发** | P2 | 当前 Executor DML 时同步分发，应优化为 COMMIT 后分发 |

---

## 十一、附录

### 11.1 审计方法
- 逐文件代码审计 + Grep 关键字验证 + cargo test 测试验证
- 三态标记：✅ 生产已启用 / 🟡 代码已实现但未启用 / ❌ 未实现

### 11.2 关键代码引用（基于代码验证）

| 功能 | 文件:行号 | 状态 |
|------|----------|------|
| CDC 接入运行时 | main.rs:831,920 | ✅ |
| MCP server 启动 | main.rs:867-877 | ✅ |
| 多节点集群模式 | main.rs:691-790 | ✅ |
| WAL writer 注入 | main.rs:922 | ✅ |
| **HTTP API 端点已启用** | main.rs:948-954, http.rs:345/632 | ✅（v7.0 修正） |
| **BufferPool 持久化接入** | main.rs:633-662 | ✅（OPT-3，v7.0 修正） |
| **CredentialStore/SCRAM 接入** | main.rs:401-440 | ✅（OPT-4） |
| BTree tuple_id u32 | btree.rs:153 | ✅ |
| InMemoryTable 移除 u16 限制 | executor.rs:1227 | ✅ |
| WalRowChange | wal.rs:343 | ✅ |
| Executor DML 接入 WAL | executor.rs:3094/3129/3150/3172 | ✅ |
| Executor DML 接入 CDC | executor.rs:3681/3691/7462/7595 | ✅ |
| slot fsync | slot.rs:349-380 | ✅ |
| API Key 256bit | service.rs:32-38 | ✅ |
| **target mysql SQL 拼接** | target/mysql.rs:134,167,373 | ❌ 非参数化 |
| **target postgres SQL 拼接** | target/postgres.rs:159,195 | ❌ 非参数化 |
| **source pg_real 真实驱动** | source/pg_real.rs:79,100,116 | ✅ rust-postgres |

### 11.3 v7.0 修订说明

**v6.0 → v7.0 主要修正**：
1. **修正 P8-2 HTTP API 层状态**：从"🟡 代码已实现，main.rs 未启用"改为"✅ main.rs:948-954 已调用 with_cdc_service"（v6.0 误标，实际已接入）
2. **新增 OPT-3 BufferPool 持久化接入记录**：main.rs:633-662 为每张表调用 `enable_persistence`（v6.0 漏报）
3. **新增 OPT-4 CredentialStore/SCRAM 接入记录**：main.rs:401-440 支持 `--auth-mode scram`
4. **修正 H3 高严重性问题状态**：从"❌ 未修复"改为"✅ 已修复"
5. **整体就绪度从 55-65% 上调为 65-72%**（反映 H3 修复 + OPT-3 接入）
6. **HTTP API 层就绪度从 40% 上调为 85%**
7. **BufferPool 就绪度从 70% 上调为 80%**
8. **CDC 引擎就绪度从 75% 上调为 80%**
9. **生产运行时 CDC 就绪度从 75% 上调为 80%**
10. **P0 待办项从 3 项减少为 2 项**（HTTP API 接入已完成）

### 11.4 当前剩余 P0 级阻塞项（2 项）

| 序号 | 任务 | 涉及文件 | 修复方案 |
|------|------|---------|---------|
| 1 | 真实数据库驱动集成 | target/mysql.rs, postgres.rs, oracle.rs, sqlserver.rs, kafka.rs | 集成 sqlx（MySQL/PG/Oracle）、tiberius（SQL Server）、rdkafka（Kafka） |
| 2 | SQL 参数化 | target/mysql.rs, postgres.rs, oracle.rs, sqlserver.rs | 改造 `SqlExecutor` trait 接收 `(sql, params)` 而非单 `&str` |

---

> **文档结束**
> 本文档基于 2026-07-31 的代码审计全面重写（v7.0），如实反映 szrsql-cdc 当前状态。
> 三态标记体系确保"已有/缺失"清晰可辨。后续代码变更后需重新审计更新。
