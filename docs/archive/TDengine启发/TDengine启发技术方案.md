# TDengine 启发技术方案 — szrsql 数据基座演进

> **来源**：涛思数据陶建辉《AI 时代的工业数据基座》战略思考
> **适用项目**：szrsql（Rust 通用 SQL 数据库）
> **创建日期**：2026-07-28
> **状态**：P1 实施中

---

## 一、背景

TDengine 文章核心论点：AI 时代，"低壁垒的功能产品"将失去价值，未来软件形态将从
`用户 → UI → 应用系统 → 数据库` 演进为 `用户/Agent → Agent Interface → 数据基座`。

虽然 TDengine 是时序数据库，但其**产品哲学层面**的判断对通用数据库 szrsql 有 5 条
可落地的启发。每条均对照 szrsql 真实代码做了校验，非主观臆测。

---

## 二、szrsql 现状（代码验证）

| 维度 | 代码事实 |
|------|---------|
| 规模 | 22 crate，v1.0.0-rc.1，5 方言 L2 协议级兼容 |
| 性能 | INSERT 比 PG 18 快 4.5x，SELECT 快 4.2x |
| AI crate | `szrsql-ai` 含 9 个子模块（mcp_server/nl2sql/rag/auto_ops 等） |
| MCP Server | 已实现 26 工具 × 7 类别 |
| Catalog 语义层 | information_schema + system_tables + multitenant + rbac + rls |
| 自治运维 | 4 类异常检测 + 线性回归容量预测 |
| RAG | 命名空间隔离 + 引用追踪 + LLM 缓存 |

**关键发现**：TDengine 启发中提到的"MCP 接口"、"分析洞察"、"语义层"在 szrsql 中已有
相当成熟的基础，但存在 3 处明确差距（见下文）。

---

## 三、五条启发与代码对照

### 启发 1：从"功能型产品"到"数据基座"

**文章原话**："AI 不会让软件产品消失，AI 会让'低壁垒的功能产品'失去存在价值。"

**代码差距**：
- `dialect.rs:482-485` — `COMMENT ON` 被正则替换为 `SELECT 1;`，纯占位无实现
- `ast.rs:1092` — `ColumnDefinition` 结构体没有 `comment` 字段
- LLM 能"读取"表结构，但无法"理解"字段业务含义

**对应优先级**：P2（COMMENT ON 真实实现）

### 启发 2：三层能力模型

| 层级 | TDengine 实现 | szrsql 真实状态 |
|------|--------------|----------------|
| 第一层：数据基础设施 | TSDB | ✅ 已夯实（B+Tree + MVCC + WAL + Raft） |
| 第二层：数据语义层 | IDMP | ⚠️ 数据目录已有，标准化/情景化缺失 |
| 第三层：分析洞察层 | 预测/异常/根因 | ⚠️ 异常检测已有，根因分析缺失 |

**对应优先级**：P2（第二层）+ P3/P4（第三层）

### 启发 3：从"展示数据"到"洞察数据"

**文章原话**："传统可视化关注的是'展示'，而 IDMP 关注的是'洞察'"。

**代码差距**：26 个 MCP 工具中 18 个是运维类，仅 `execute_sql` 让 LLM 拿到原始数据。
缺少"自动数据摘要"和"根因分析"这类洞察类工具。

**对应优先级**：P1（summarize_table + ask_data）+ P4（explain_root_cause）

### 启发 4：开放系统

**代码现状**：
- ✅ MCP（26 工具）
- ✅ CDC + Debezium JSON/Avro
- ⚠️ 缺少 MQTT 直连
- ⚠️ 缺少实时数据流订阅原生协议

**对应优先级**：暂列未来规划

### 启发 5：Agent Interface + 数据基座

**文章原话**："未来很多工业软件，不再会是传统意义上的应用，而是运行在工业数据基座之上的 AI Agent。"

**代码差距**：`rag.rs` 的 `rag_ask()` 未通过 MCP 暴露统一入口。LLM 必须三步走
（list_tables → describe_table → execute_sql），而非一句话 `ask_data()` 拿洞察。

**对应优先级**：P1（ask_data 统一入口）

---

## 四、实施方案 P1-P5

### P1：MCP 新增 Insight 类别工具（summarize_table + ask_data）

**目标**：在现有 26 工具基础上新增 2 个洞察类工具，新增 `Insight` 类别。
复用 `rag.rs` / `nl2sql.rs` / `auto_ops.rs` 已有能力。

**改动文件**：`crates/szrsql-ai/src/mcp_server.rs`

**新增 DTO**：
- `ColumnSummary` — 列级统计（基数/NULL 数/min/max/top 值）
- `TableSummary` — 表级摘要（行数 + 各列统计）
- `AskAnswer` — 自然语言问答结果（答案 + SQL + 引用）

**新增工具**：
- `summarize_table` — 输入表名，返回自动数据摘要
- `ask_data` — 输入自然语言问题，返回答案 + SQL + 引用

**门禁**：纯 szrsql-ai crate 内部扩展，不触发差分比对。

### P2：COMMENT ON 真实实现 + ColumnDefinition 增加 comment 字段

**目标**：将 `dialect.rs` 的 COMMENT 占位升级为真实功能。

**改动文件**：
- `crates/szrsql-sql/src/ast.rs` — `ColumnDefinition` 新增 `comment` 字段
- `crates/szrsql-sql/src/parser.rs` — 新增 `convert_comment`
- `crates/szrsql-sql/src/dialect.rs` — 删除 COMMENT 占位正则
- `crates/szrsql-sql/src/executor.rs` — 新增 `execute_comment`
- `crates/szrsql-catalog/src/lib.rs` — `MutableCatalog` 新增 comment 方法
- `crates/szrsql-catalog/src/information_schema.rs` — `columns` 视图新增 COMMENT 列
- `crates/szrsql-ai/src/mcp_server.rs` — `ColumnDef` DTO 新增 comment

**门禁**：触发差分模糊测试 + 变异测试（parser.rs）。

### P3：新增 time_bucket() 时序分析函数

**目标**：对标 TimescaleDB `time_bucket('1 hour', ts)`。

**改动文件**：
- `crates/szrsql-sql/src/ast.rs` — 新增 `BuiltinFunction::TimeBucket`
- `crates/szrsql-sql/src/executor.rs` — 新增 `eval_time_bucket`
- `crates/szrsql-types/src/value.rs` — 新增 `Interval` 类型（如未存在）

**门禁**：触发差分模糊测试 + 变异测试（executor.rs）。

### P4：MCP 新增 explain_root_cause 根因分析工具

**目标**：关联 alerts + slow_queries + wait_events 三源数据，生成根因报告。

**改动文件**：`crates/szrsql-ai/src/mcp_server.rs`

**新增 DTO**：
- `RootCauseReport` — 根因报告（告警 + 可能原因 + 证据）
- `CauseEntry` — 单条原因（类型 + 描述 + 置信度）
- `Evidence` — 证据（来源 + 详情）

**根因推理规则**：
- 全表扫描告警 → MissingIndex（置信度 0.8）
- 超时 + 等待事件占比高 → LockContention（置信度 0.6）
- 死锁告警 → Deadlock（置信度 0.9）
- 高错误率 → StatsStale（置信度 0.5）

**门禁**：纯 szrsql-ai crate，不触发差分比对。

### P5：数据血缘追踪（最小可行版）

**目标**：字段级血缘，记录上游来源。不引入图数据库依赖。

**改动文件**：
- `crates/szrsql-catalog/src/lineage.rs` — 新建文件
- `crates/szrsql-catalog/src/lib.rs` — 新增 `lineage` 模块
- `crates/szrsql-cdc/src/schema.rs` — Schema 演化时记录血缘
- `crates/szrsql-sql/src/executor.rs` — CTAS 时记录血缘
- `crates/szrsql-ai/src/mcp_server.rs` — 新增 `get_lineage` 工具

**门禁**：触发变异测试（lineage.rs）。

---

## 五、执行顺序

```
P1（MCP summarize/ask_data）        — 零 SQL 逻辑变更，最低风险
  ↓
P4（MCP explain_root_cause）        — 同 MCP 层，复用 P1 模式
  ↓
P2（COMMENT ON 真实实现）           — 触发 SQL 逻辑变更，需差分比对
  ↓
P3（time_bucket 函数）              — 触发 SQL 逻辑变更，需差分比对
  ↓
P5（数据血缘）                       — 大改动，最后做
```

---

## 六、门禁清单

遵循 `.trae/rules/project_rules.md` 与 `AGENTS.md`：

| 门禁 | P1 | P2 | P3 | P4 | P5 |
|------|----|----|----|----|---|
| `cargo check --workspace` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `cargo clippy -D warnings` | ✅ | ✅ | ✅ | ✅ | ✅ |
| 受影响 crate 全量测试 | szrsql-ai | szrsql-sql + catalog + ai | szrsql-sql + types | szrsql-ai | catalog + cdc + sql |
| 差分比对（PG 18） | — | ✅ | ✅ | — | — |
| 变异测试 ≥ 95% | — | ✅ parser.rs | ✅ executor.rs | — | ✅ lineage.rs |
| unwrap/expect 审查 | ✅ | ✅ | ✅ | ✅ | ✅ |
| `@REVIEW_REQUIRED` | — | executor | executor | — | — |
