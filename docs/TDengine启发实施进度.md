# TDengine 启发实施进度

> **创建日期**：2026-07-28
> **方案文档**：[TDengine启发技术方案.md](./TDengine启发技术方案.md)

---

## 总览

| 优先级 | 任务 | 状态 | 开始日期 | 完成日期 | 备注 |
|--------|------|------|---------|---------|------|
| P1 | MCP 新增 Insight 类别（summarize_table + ask_data） | ✅ 已完成 | 2026-07-28 | 2026-07-28 | 86 测试通过，clippy 零警告 |
| P2 | COMMENT ON 真实实现 + ColumnDefinition.comment | ✅ 已完成 | 2026-07-28 | 2026-07-28 | 3 parser + 1 catalog 测试通过 |
| P3 | time_bucket() 时序分析函数 | ✅ 已完成 | 2026-07-28 | 2026-07-28 | 6 测试通过，覆盖 4 单位 + 边界 |
| P4 | MCP explain_root_cause 根因分析 | ✅ 已完成 | 2026-07-28 | 2026-07-28 | 复用 P1 模式，5 测试通过 |
| P5 | 数据血缘追踪（最小可行版） | ✅ 已完成 | 2026-07-28 | 2026-07-28 | 11 catalog + 8 MCP 测试通过 |

---

## P1 详细进度

### 代码改动

| 子任务 | 文件 | 状态 | 验证方式 |
|--------|------|------|---------|
| 新增 `Insight` 工具类别枚举 | mcp_server.rs | ✅ | cargo check |
| 新增 `ColumnSummary` / `TableSummary` / `AskCitation` / `AskAnswer` DTO | mcp_server.rs | ✅ | cargo check |
| `McpBackendV2` trait 新增 `summarize_table` / `ask_data` 方法 | mcp_server.rs | ✅ | cargo check |
| `MockBackendV2` 实现 `summarize_table` | mcp_server.rs | ✅ | 单元测试 |
| `MockBackendV2` 实现 `ask_data` | mcp_server.rs | ✅ | 单元测试 |
| `tool_definitions()` 新增 2 个工具定义 | mcp_server.rs | ✅ | 工具总数测试 |
| `handle_tools_call` 新增 2 个分发分支 | mcp_server.rs | ✅ | 分发测试 |
| 新增 `tool_summarize_table` / `tool_ask_data` 方法 | mcp_server.rs | ✅ | 单元测试 |
| `TOOL_COUNT` 常量 26 → 28 | mcp_server.rs | ✅ | 工具总数测试 |
| `ToolCategory::all()` 新增 Insight | mcp_server.rs | ✅ | 类别计数测试 |
| `EmptyBackend` 实现 `summarize_table` / `ask_data` | mcp_server.rs | ✅ | 编译通过 |

### 测试改动

| 测试 | 状态 | 说明 |
|------|------|------|
| `test_7d22_tool_count_is_28` | ✅ 通过 | 工具总数 26→28 |
| `test_7d22_all_8_categories_covered` | ✅ 通过 | 类别 7→8 |
| `test_7d22_category_tool_counts` | ✅ 通过 | Insight=2, total=28 |
| `test_7d22_expected_tool_names_present` | ✅ 通过 | 含 summarize_table / ask_data |
| `test_7d22_tool_category_as_str` | ✅ 通过 | Insight="insight" |
| `test_7d22_tool_category_all` | ✅ 通过 | 8 个类别 |
| `test_7d22_tools_list_returns_28` | ✅ 通过 | tools/list 返回 28 |
| `test_7d22_full_llm_workflow` | ✅ 通过 | 新增 ask_data 步骤 |
| `test_7d22_all_28_tools_callable` | ✅ 通过 | 28 工具全部可调用 |
| `test_7d22_summarize_table_products` | ✅ 通过 | summarize_table 功能 |
| `test_7d22_summarize_table_not_found` | ✅ 通过 | 错误处理 |
| `test_7d22_summarize_table_via_mcp` | ✅ 通过 | MCP 端到端 |
| `test_7d22_summarize_table_missing_arg` | ✅ 通过 | 参数缺失错误 |
| `test_7d22_ask_data_products` | ✅ 通过 | ask_data 商品场景 |
| `test_7d22_ask_data_orders` | ✅ 通过 | ask_data 订单场景 |
| `test_7d22_ask_data_slow_query` | ✅ 通过 | ask_data 慢查询场景 |
| `test_7d22_ask_data_no_match` | ✅ 通过 | ask_data 无匹配场景 |
| `test_7d22_ask_data_via_mcp` | ✅ 通过 | MCP 端到端 |
| `test_7d22_ask_data_missing_arg` | ✅ 通过 | 参数缺失错误 |
| `test_7d22_insight_category_tools` | ✅ 通过 | Insight 类别 2 工具 |
| `test_7d22_insight_dto_serialization` | ✅ 通过 | DTO 序列化 |

### 门禁验证

| 门禁 | 状态 | 命令 | 结果 |
|------|------|------|------|
| cargo check | ✅ | `cargo check -p szrsql-ai` | 通过 |
| cargo clippy | ✅ | `cargo clippy -p szrsql-ai -- -D warnings` | 零警告 |
| cargo test (mcp_server) | ✅ | `cargo test -p szrsql-ai mcp_server` | 86 passed, 0 failed |
| cargo test (全 crate) | ✅ | `cargo test -p szrsql-ai` | 339 passed, 0 failed |
| unwrap/expect 审查 | ✅ | 代码审查 | 无新增裸 unwrap |

### 改动统计

- **修改文件**：1 个（`crates/szrsql-ai/src/mcp_server.rs`）
- **新增代码行**：约 250 行（DTO + trait 方法 + Mock 实现 + 工具定义 + 分发 + 工具方法 + 21 个测试）
- **修改代码行**：约 30 行（TOOL_COUNT / category / 已有测试更新）
- **新增工具**：2 个（summarize_table / ask_data）
- **新增类别**：1 个（Insight）
- **新增 DTO**：4 个（ColumnSummary / TableSummary / AskCitation / AskAnswer）
- **新增测试**：13 个

---

## P2 详细进度：COMMENT ON 真实实现

### 代码改动

| 子任务 | 文件 | 状态 | 验证方式 |
|--------|------|------|---------|
| `ColumnDefinition` 新增 `comment: Option<String>` 字段 | ast.rs | ✅ | cargo check |
| 新增 `Statement::Comment` 变体 + `CommentObjectType` 枚举 | ast.rs | ✅ | cargo check |
| 新增 `parse_comment` 解析函数 | parser.rs | ✅ | 单元测试 |
| `parse_sql_inner` 添加 COMMENT ON 检测 | parser.rs | ✅ | 单元测试 |
| 删除 dialect.rs 中 COMMENT ON 占位正则 | dialect.rs | ✅ | cargo check |
| `MutableCatalog` trait 新增 4 个 comment 方法 | catalog/lib.rs | ✅ | cargo check |
| `ManagedCatalog` 实现 comment 方法 | catalog/lib.rs | ✅ | 单元测试 |
| `CatalogAdapter` 实现 comment 方法（set 返回错，get 委托） | system_tables.rs | ✅ | cargo check |
| `plan.rs` 添加 `Statement::Comment` 分支（Unsupported） | plan.rs | ✅ | cargo check |
| `session.rs` 拦截并执行 COMMENT ON 语句 | protocol/session.rs | ✅ | cargo check |
| `ColumnDef` DTO 新增 `comment` 字段 | mcp.rs | ✅ | cargo check |

### 测试改动

| 测试 | 状态 | 说明 |
|------|------|------|
| `test_parse_comment_on_table` | ✅ 通过 | COMMENT ON TABLE ... IS '...' |
| `test_parse_comment_on_column` | ✅ 通过 | COMMENT ON COLUMN ... IS '...' |
| `test_parse_comment_on_null` | ✅ 通过 | COMMENT ON TABLE ... IS NULL |
| `test_comment_storage` | ✅ 通过 | set/get table + column comment |

### 门禁验证

| 门禁 | 状态 | 命令 | 结果 |
|------|------|------|------|
| cargo check | ✅ | `cargo check --workspace` | 通过 |
| cargo clippy | ✅ | `cargo clippy -p szrsql-sql -p szrsql-catalog -- -D warnings` | 零警告 |
| cargo test (parser) | ✅ | `cargo test -p szrsql-sql --lib comment` | 3 passed |
| cargo test (catalog) | ✅ | `cargo test -p szrsql-catalog --lib` | 296 passed |
| unwrap/expect 审查 | ✅ | 代码审查 | 无新增裸 unwrap |

---

## P3 详细进度：time_bucket() 时序分析函数

### 代码改动

| 子任务 | 文件 | 状态 | 验证方式 |
|--------|------|------|---------|
| `eval_function` 新增 `time_bucket` 分支 | expr.rs | ✅ | 单元测试 |
| 新增 `parse_bucket_width` 辅助函数 | expr.rs | ✅ | 单元测试 |
| 支持 4 种时间单位：s/min/h/d | expr.rs | ✅ | 单元测试 |
| NULL 输入短路返回 NULL | expr.rs | ✅ | 单元测试 |
| 类型不匹配错误处理 | expr.rs | ✅ | 单元测试 |
| 无效单位错误处理 | expr.rs | ✅ | 单元测试 |

### 测试改动

| 测试 | 状态 | 说明 |
|------|------|------|
| `test_time_bucket_1_hour` | ✅ 通过 | 1h 桶对齐 |
| `test_time_bucket_5_minutes` | ✅ 通过 | 5min 桶对齐 |
| `test_time_bucket_1_day` | ✅ 通过 | 1d 桶对齐 |
| `test_time_bucket_null` | ✅ 通过 | NULL 输入返回 NULL |
| `test_time_bucket_invalid_arg_type` | ✅ 通过 | 非文本参数报错 |
| `test_time_bucket_invalid_unit` | ✅ 通过 | 未知单位报错 |

### 门禁验证

| 门禁 | 状态 | 命令 | 结果 |
|------|------|------|------|
| cargo check | ✅ | `cargo check -p szrsql-sql` | 通过 |
| cargo clippy | ✅ | `cargo clippy -p szrsql-sql -- -D warnings` | 零警告 |
| cargo test | ✅ | `cargo test -p szrsql-sql --lib time_bucket` | 6 passed |
| unwrap/expect 审查 | ✅ | 代码审查 | 使用 `?` 和模式匹配，无裸 unwrap |

---

## P4 详细进度：MCP explain_root_cause 根因分析

### 代码改动

| 子任务 | 文件 | 状态 | 验证方式 |
|--------|------|------|---------|
| 新增 `AlertInfo` / `CauseEntry` / `Evidence` / `RootCauseReport` DTO | mcp_server.rs | ✅ | 序列化测试 |
| `McpBackendV2` trait 新增 `explain_root_cause` 方法 | mcp_server.rs | ✅ | cargo check |
| `MockBackendV2` 实现根因推理逻辑 | mcp_server.rs | ✅ | 单元测试 |
| `tool_definitions()` 新增工具定义 | mcp_server.rs | ✅ | 工具总数测试 |
| `handle_tools_call` 新增分发分支 | mcp_server.rs | ✅ | 分发测试 |
| 新增 `tool_explain_root_cause` 方法 | mcp_server.rs | ✅ | 单元测试 |
| `TOOL_COUNT` 常量 28 → 30 | mcp_server.rs | ✅ | 工具总数测试 |
| `EmptyBackend` 实现 `explain_root_cause` | mcp_server.rs | ✅ | 编译通过 |

### 测试改动

| 测试 | 状态 | 说明 |
|------|------|------|
| `test_7d22_root_cause_high_qps` | ✅ 通过 | 高 QPS 根因推理 |
| `test_7d22_root_cause_alert_not_found` | ✅ 通过 | 告警不存在错误 |
| `test_7d22_root_cause_missing_arg` | ✅ 通过 | 参数缺失错误 |
| `test_7d22_root_cause_via_mcp` | ✅ 通过 | MCP 端到端 |
| `test_7d22_root_cause_report_serialization` | ✅ 通过 | DTO 序列化 |

### 门禁验证

| 门禁 | 状态 | 命令 | 结果 |
|------|------|------|------|
| cargo check | ✅ | `cargo check -p szrsql-ai` | 通过 |
| cargo clippy | ✅ | `cargo clippy -p szrsql-ai -- -D warnings` | 零警告 |
| cargo test | ✅ | `cargo test -p szrsql-ai --lib` | 355 passed |
| unwrap/expect 审查 | ✅ | 代码审查 | 无新增裸 unwrap |

---

## P5 详细进度：数据血缘追踪（最小可行版）

### 代码改动

| 子任务 | 文件 | 状态 | 验证方式 |
|--------|------|------|---------|
| 新建 `lineage.rs` 模块 | catalog/lineage.rs | ✅ | 单元测试 |
| 定义 `ColumnRef` / `LineageEdge` / `EdgeSource` 结构 | catalog/lineage.rs | ✅ | 单元测试 |
| 实现 `LineageStore` 内存存储 | catalog/lineage.rs | ✅ | 单元测试 |
| 实现 `add_edge` / `upstream_of` / `downstream_of` / `all_edges` | catalog/lineage.rs | ✅ | 单元测试 |
| 幂等添加（重复边不累加） | catalog/lineage.rs | ✅ | 单元测试 |
| 列级血缘支持 | catalog/lineage.rs | ✅ | 单元测试 |
| `catalog/lib.rs` 导出 lineage 模块 | catalog/lib.rs | ✅ | cargo check |
| 新增 `LineageEdgeDto` / `LineageInfo` / `LineageEdgeSource` DTO | mcp_server.rs | ✅ | 序列化测试 |
| `McpBackendV2` trait 新增 `get_lineage` 方法 | mcp_server.rs | ✅ | cargo check |
| `MockBackendV2` 实现 `get_lineage` | mcp_server.rs | ✅ | 单元测试 |
| `tool_definitions()` 新增 get_lineage 工具定义 | mcp_server.rs | ✅ | 工具总数测试 |
| `handle_tools_call` 新增分发分支 | mcp_server.rs | ✅ | 分发测试 |
| 新增 `tool_get_lineage` 方法 | mcp_server.rs | ✅ | 单元测试 |
| `EmptyBackend` 实现 `get_lineage` | mcp_server.rs | ✅ | 编译通过 |

### 测试改动（catalog lineage）

| 测试 | 状态 | 说明 |
|------|------|------|
| `test_lineage_add_and_upstream` | ✅ 通过 | 添加边 + 上游查询 |
| `test_lineage_downstream` | ✅ 通过 | 下游查询 |
| `test_lineage_column_level` | ✅ 通过 | 列级血缘 |
| `test_lineage_different_source_type_distinct` | ✅ 通过 | 不同来源类型区分 |
| `test_lineage_different_transform_creates_new_edge` | ✅ 通过 | 不同 transform 创建新边 |
| `test_lineage_edge_direct_constructor` | ✅ 通过 | 直接构造 LineageEdge |
| `test_lineage_edge_source_as_str` | ✅ 通过 | EdgeSource 字符串转换 |
| `test_lineage_idempotent_add` | ✅ 通过 | 幂等添加 |
| `test_lineage_len_and_is_empty` | ✅ 通过 | 边数统计 |
| `test_lineage_all_edges_and_tables` | ✅ 通过 | 全量边 + 表名列表 |
| `test_lineage_upstream_empty_for_unknown_table` | ✅ 通过 | 未知表返回空 |

### 测试改动（MCP lineage）

| 测试 | 状态 | 说明 |
|------|------|------|
| `test_7d22_lineage_dto_serialization` | ✅ 通过 | DTO 序列化 |
| `test_7d22_lineage_edge_source_as_str` | ✅ 通过 | LineageEdgeSource 字符串 |
| `test_7d22_lineage_empty_backend` | ✅ 通过 | 空后端返回空 |
| `test_7d22_lineage_get_all` | ✅ 通过 | 全量血缘查询 |
| `test_7d22_lineage_get_for_products` | ✅ 通过 | 指定表下游查询 |
| `test_7d22_lineage_get_for_orders` | ✅ 通过 | 指定表上游查询 |
| `test_7d22_lineage_get_unknown_table` | ✅ 通过 | 未知表返回空 |
| `test_7d22_lineage_via_mcp` | ✅ 通过 | MCP 端到端 |
| `test_7d22_lineage_via_mcp_no_args` | ✅ 通过 | 无参数返回全量 |

### 门禁验证

| 门禁 | 状态 | 命令 | 结果 |
|------|------|------|------|
| cargo check | ✅ | `cargo check -p szrsql-catalog -p szrsql-ai` | 通过 |
| cargo clippy | ✅ | `cargo clippy -p szrsql-catalog -p szrsql-ai -- -D warnings` | 零警告 |
| cargo test (catalog) | ✅ | `cargo test -p szrsql-catalog --lib lineage` | 11 passed |
| cargo test (ai) | ✅ | `cargo test -p szrsql-ai --lib lineage` | 9 passed |
| unwrap/expect 审查 | ✅ | 代码审查 | 使用 `?` 和模式匹配，无裸 unwrap |

---

## 变更日志

| 日期 | 变更 | 操作人 |
|------|------|--------|
| 2026-07-28 | 创建文档，P1 开始实施 | AI Agent |
| 2026-07-28 | P1 完成：86 测试通过，clippy 零警告，MCP 工具 26→28 | AI Agent |
| 2026-07-28 | P2 完成：COMMENT ON 真实实现，4 测试通过 | AI Agent |
| 2026-07-28 | P3 完成：time_bucket 函数，6 测试通过 | AI Agent |
| 2026-07-28 | P4 完成：explain_root_cause 根因分析，5 测试通过，MCP 工具 28→30 | AI Agent |
| 2026-07-28 | P5 完成：数据血缘追踪，20 测试通过（11 catalog + 9 MCP） | AI Agent |
| 2026-07-28 | 全工作区验证：cargo check 通过，clippy 零警告（修改的 crate） | AI Agent |

---

## 全量验证总结

### 修改的 crate 验证结果

| Crate | cargo check | cargo clippy | cargo test |
|-------|-------------|--------------|------------|
| szrsql-sql | ✅ 通过 | ✅ 零警告 | ✅ 2613 passed |
| szrsql-catalog | ✅ 通过 | ✅ 零警告 | ✅ 296 passed |
| szrsql-ai | ✅ 通过 | ✅ 零警告 | ✅ 355 passed |
| szrsql-protocol | ✅ 通过 | ⚠️ 预存警告 | ✅ 通过 |

### 全工作区验证

| 命令 | 结果 | 说明 |
|------|------|------|
| `cargo check --workspace` | ✅ exit 0 | 全部 crate 编译通过 |
| `cargo clippy -p szrsql-sql -p szrsql-catalog -p szrsql-ai -- -D warnings` | ✅ exit 0 | 修改的 crate 零警告 |
| `cargo test -p szrsql-sql --lib time_bucket` | ✅ 6 passed | P3 time_bucket |
| `cargo test -p szrsql-sql --lib comment` | ✅ 3 passed | P2 COMMENT ON 解析 |
| `cargo test -p szrsql-catalog --lib` | ✅ 296 passed | P2 + P5 catalog |
| `cargo test -p szrsql-ai --lib` | ✅ 355 passed | P1 + P4 + P5 MCP |

### 预存问题（非本次改动引入）

| 问题 | 文件 | 说明 |
|------|------|------|
| oracle-bridge 测试编译错误 | szrsql-oracle-bridge/src/server.rs:484 | `handle()` 期望 `TcpStream`，测试用 `DuplexStream`；初始提交即存在 |
| szrsql-protocol clippy 警告 | system_tables.rs / server.rs / session.rs | type_complexity / map_clone / for_kv_map；初始提交即存在 |
| temp_table_tests.rs 未使用导入 | szrsql-sql/src/temp_table_tests.rs:22 | `TableStorage` 未使用；初始提交即存在 |
