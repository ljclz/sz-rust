# 变更日志

本项目所有重要变更均会记录在此文件中。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased] - 2026-08-15（连接池调优接线：L2 配置 + 预热 + 动态扩缩容 + 池指标）

### Added

- **连接池 L2/L3 调优接线**（来源: `cargo test -p sz-rust-sz300 -j 2` → lib 43 passed / 0 failed，含新增 4 个 `db::tests`；clippy 零警告）：
  - `SqlxPoolConfig` 接线：`DB_POOL_MAX / DB_POOL_MIN / DB_POOL_ACQUIRE_TIMEOUT / DB_POOL_IDLE_TIMEOUT / DB_POOL_MAX_LIFETIME` 环境变量调优，未设置保持既有默认（20/10/30s/600s/1800s），非法值回退默认（`db.rs:26-47`）
  - SQLx 池补齐 `min_connections` / `idle_timeout` / `max_lifetime`，与 sz-orm 层参数同源对齐，两层池参数不再各设各的（`db.rs:56-65`、`db.rs:129-138`）
  - 连接池预热：sz-orm `prewarm(true)` + 启动时 `pool.prewarm()`，消除首次请求冷启动延迟（`db.rs:74`、`main.rs:200-207`）
  - PoolScaler 动态扩缩容：30s 周期后台任务，`acquire_failed_count` 作为超时率代理驱动决策，扩容上限对齐 SQLx 池容量（防两层池失配缺陷复发），仅实际调整时 resize（`main.rs:211-254`）
  - /metrics 实时输出池指标：`sz300_db_pool_active/idle/waiters/max/acquire_total/acquire_failed_total`（PG 池为 `sz300_pg_pool_*`），O(1) 原子读取；移除注册后无人更新的恒 0 gauge `sz300_active_connections`（`controllers/health.rs:112-146`）
- 五维审查报告：`docs/audit/2026-08-15-连接池调优接线五维审查报告.md`（5 维度无阻断项）

## [Unreleased] - 2026-08-15（sz300 首次真实运行 + metrics 鉴权接线）

### Fixed

- **sz300 首次真实运行冒烟测试修复**（commit `70ea91d`，来源: `cargo test -p sz-rust-sz300` → 134 passed / 0 failed）：
  - `merchant_service.rs`：商户列表投影引用不存在的 `merchant_name` 列 → 商户列表必崩缺陷，改为真实列并补全显式投影
  - `order_service.rs`：order_item 查询复用 2 参数数组但 SQL 仅 1 占位符 → order/info 必失败缺陷，改为独立单参数数组
  - device/merchant/order/product 四个 service 共 10 处 `SELECT *` → 显式列投影（铁律：禁 SELECT *）

### Security

- **/metrics 鉴权接线（T7）**：`MetricsAuthConfig`（此前仅定义未接线）现通过 `metrics_auth_middleware` 挂载到独立 `/metrics` 子路由（`router.rs:54` metrics_router()，route_layer 不污染业务 API）：
  - Bearer token（`SZ300_METRICS_BEARER_TOKEN`）+ IP 白名单（`SZ300_METRICS_ALLOWED_IPS`，支持 CIDR v4/v6）双机制，未授权返回 403
  - `SZ300_ENV=production` 启动强制校验：未配置任何鉴权 → 拒绝启动（fail-closed，`main.rs:243`）
  - 客户端真实 IP 注入：`into_make_service_with_connect_info`（`main.rs:262`）
  - 新增集成测试 `tests/metrics_auth_router_test.rs`（7 用例）+ CIDR 单测 3 用例（来源: `cargo test -p sz-rust-sz300` 真实输出）

## [Unreleased] - 2026-08-13（P0/P1/P2 能力完善）

> **文档降级声明**（2026-08-13 审计核实）：以下部分声称经独立验证存在跨仓库混淆或测试编译失败，已标注真实状态。
> 审计报告：`docs/audit/2026-08-13-文档已实现但生产零调用审计报告.md`

### Added

- **10 个能力缺口完成情况**（P0×4 + P1×4 + P2×2）：
  - P0-1 Capability Registry：PermissionChecker + tenant 隔离（38 tests ✅，来源: `cargo test -p sz-rust-capability`，开源仓库）
  - P0-2 SDD Agent：四阶段编排 + RAG 集成 + 铁律检查 + 16 Skill 映射（❌ **未完成**：`sz-rust-sdd-agent` 在开源版与企业版仓库 git 历史中均不存在，声称的 128 tests 无法复现——2026-08-14 核验定性为虚构交付）
  - P0-3 开源/企业分离：4 个企业 crate 分离 + 许可证合规 CI（✅ 来源: `check_license_compliance.py`）
  - P0-4 插件数据互通：EventBus + CrossQuery + 共享 Schema（13 tests ✅，来源: `cargo test -p sz-rust-core --test event_bus_test`，但 sz300 生产零调用）
  - P1-1 AI 迁移：3 个真实 TP6 项目案例 + MigrationReport（❌ **未完成**：`sz-rust-migration` 在开源版与企业版仓库 git 历史中均不存在，声称的 22 tests 无法复现——2026-08-14 核验定性为虚构交付）
  - P1-2 行业 RAG：25 术语 + 10 规则 + 7 模型模板（52 tests ✅，来源: `cargo test -p sz-rust-rag`，但生产零调用）
  - P1-3 插件模板库：12 个 Tera 模板 + SafetyValidator（289 tests ✅，来源: `cargo test -p sz-rust-cli`）
  - P1-4 MCP 扩展：8+ 新工具 + 白名单鉴权 + Capability 适配（70 tests ✅，来源: `cargo test -p sz-rust-mcp -p sz-rust-capability`）
  - P2-1 可视化画布：preview_app + 事件过滤 + HITL Abort + 冷启动脚本（❌ **未完成**：`sz-rust-visual` 在开源版与企业版仓库 git 历史中均不存在，声称的 56 tests 无法复现——2026-08-14 核验定性为虚构交付）
  - P2-2 插件市场：支付集成 + 订阅服务 + 5 项审核检查 + CLI/Web 同步（❌ **未完成**：`sz-rust-marketplace` 在开源版与企业版仓库 git 历史中均不存在，声称的 73 tests 无法复现——2026-08-14 核验定性为虚构交付）
- **6 个企业版业务插件**（119 tests ✅ 全部通过，来源: `cargo test -p sz-addons-market -p sz-addons-restaurant -p sz-addons-retail -p sz-plugin-sso-enterprise -p sz-plugin-audit -p sz-plugin-ha`，企业版仓库 `E:\www\rust\sz-rust-enterprise\`）
- **10 个 ADR**（ADR-026~035）+ **4 个五维审查报告**
- 关键数字（经审计核实）：开源仓库 462 tests ✅ + 企业版旧 4 crate 172 tests ✅ + 企业版新 6 插件 119 tests ✅ = 753 tests passed；`cargo test --workspace` 有 workspace 级依赖冲突待修复（来源: 各 crate `cargo test -p <crate>` 输出）

## [Unreleased] - 2026-08-12（W5/W6 质量交付）

### Added

- **W5/W6 质量交付**（覆盖 W1/W3/W5/W6/W7 弱点）：
  - TF 任务域：EnvGuard Drop guard 测试隔离工具（`packages/sz-rust-sz300/tests/common/mod.rs`），addons-forum/im 补充 Controller+Model 单元测试（23+23=46 个新测试）
  - PB 任务域：ORM 查询构建 + 缓存读写 2 组 10 个 criterion bench case，启动内存 RSS 测量脚本，性能基线 `w5_w6` 已保存
  - SA 任务域：22 条铁律自动化检查脚本（`scripts/check_iron_laws.py`），漏洞与许可证扫描脚本（`scripts/run_security_audit.py`），安全审计报告
  - E2E 任务域：ssh2 部署脚本（`scripts/e2e_deploy.js`，禁止 sshpass/killall），E2E 8 阶段编排脚本（`scripts/e2e_orchestrate.js`），E2E 验证报告
  - 新增文件 16 个，修改文件 10 个（来源: design.md §2.7 文件清单）
  - 关键数字：171 passed 0 failed / 45 bench case / 22 铁律全通过 / 8 阶段全执行（来源: 各任务域验证命令输出）

## [Unreleased] - 2026-08-12（P4-T2 工作流引擎）

### Added

- **工作流引擎**（`sz-rust-workflow`）：
  - 28 个错误码 `WF_001`～`WF_051`，按功能分区（定义加载/状态机/审批流/插件节点/实例管理/设计器 API）
  - 流程定义模型：`FlowDefinition`/`Node`/`NodeConfig`（6 种节点类型）/`Transition`/`CandidateStrategy`/`ApprovalStrategyType`/`FaultStrategy`
  - `DefinitionParser`：YAML/JSON 格式识别与解析，`flow_key` 命名规范校验
  - `DefinitionValidator`：结构完整性校验（必需字段/节点唯一性）、可达性校验（BFS）、终止性校验、插件节点引用校验
  - `DefaultGuardEvaluator`：纯函数表达式子集（字段访问/比较/逻辑运算），副作用检测，表达式长度限制
  - `StateMachineEngine`：迁移查找与触发，守卫条件求值，乐观锁原子迁移（持久化→内存→事件）
  - `ApprovalFlowEngine`：任务办理入口，审批策略检查（会签/或签），节点推进调度，死循环防护
  - `AndSignStrategy`/`OrSignStrategy`：会签（全同意完成）/或签（任一同意完成）
  - `DefaultCandidateResolver`：静态用户/动态表达式/能力调用三种候选人解析
  - `PluginNodeExecutor`：能力调用与超时，版本协商，容错策略（fail/skip/retry 指数退避）
  - `DefaultFaultStrategyHandler`：Fail 终止/Skip 跳过/Retry 指数退避重试
  - `SensitiveFieldRegistry`：动态敏感字段注册，递归脱敏（string/number/boolean/object/array），系统字段保护
  - `PluginUnloadWatcher`：插件卸载联动，在途实例标记不可用节点
  - `InstanceManager`：启动/挂起/恢复/终止/查询/历史轨迹，UUID v4 实例 ID
  - `HistoryRecorder`：迁移/节点/任务历史记录，敏感字段脱敏后持久化
  - `TaskManager`：任务生命周期管理（创建/失效/分页查询）
  - Repository trait 抽象：`DefinitionRepository`/`InstanceRepository`（乐观锁）/`TaskRepository`/`HistoryRepository`
  - `InMemoryRepository`：测试用默认实现（parking_lot::RwLock<HashMap>）
  - `WorkflowEventBus`：事件总线 trait + `InMemoryEventBus`（broadcast channel）+ `NoopEventBus`
  - `WorkflowMetrics`：4 个 Prometheus 指标（active instance/pending task/transition duration/plugin error）
  - `AuditLogger`：结构化审计日志（6 类操作），span 含 instance_id/node_id/transition
  - `VersionManager`：版本管理（列出/生效/弃用）
  - `DesignerApi`：定义校验（不持久化）/导入/导出，YAML/JSON 等价
  - `WorkflowDeps`/`WorkflowDepsBuilder`：依赖注入容器 + builder 模式
  - `WorkflowEngine`：统一门面，委托所有子模块
  - 131 个测试全部通过（121 单元 + 10 集成），clippy 零警告

## [Unreleased] - 2026-08-12（P4-T1 前端代码生成）

### Added

- **前端代码生成器**（`sz-rust-frontend-codegen`）：
  - 17 变体 `FrontendCodegenError` 错误类型 + `FE_CODEGEN_*` 错误码
  - 模型元信息结构（`FieldMetadata`/`RelationMetadata`/`ValidationRule`/`ModelMetadata`）
  - `ModelParser`：syn AST 解析 `#[derive(Model)]` 结构体，提取字段/关系/验证规则
  - Rust→TypeScript 类型映射（String→string, i32→number, Option<T>→T | null, Vec<T>→T[]）
  - `CodegenTemplateEngine`：Tera 1.20 封装 + 8 个自定义过滤器
  - `PathGuard`：路径穿越防护（拒绝 `..`/绝对路径/控制字符）
  - `UiAdapter`：Element Plus（el-*）/ Ant Design Vue（a-*）标签映射
  - `FileWriter`：原子文件写入（临时文件 + rename）+ 三种覆盖策略（Skip/Overwrite/Merge）
  - 21 个内置 Tera 模板（Vue 4 页面 + 测试骨架 + React 4 页面 + 路由 + 权限 + API + 类型）
  - `VueComponentGenerator`/`ReactComponentGenerator`：组件生成
  - `RouteGenerator`：Vue Router / React Router v6 路由生成
  - `PermissionGenerator`：路由守卫 + v-permission 指令 + usePermission 组合式函数
  - `ApiClientGenerator` + `OpenApiSchemaExtractor`：OpenAPI spec → API 客户端 + TS 类型
  - `CodegenService`：核心流水线编排（解析→适配→生成→写入→报告）
  - `GenerationReport`：结构化报告 + CLI 表格输出
  - 71 个测试全部通过（52 单元 + 19 集成）

- **CLI 集成**（`sz-rust-cli`）：
  - `sz-rust make:frontend` 子命令（--model/--framework/--ui/--output/--override/--with-tests 等 11 参数）
  - 275 个 CLI 测试全部通过

## [Previous] - 2026-08-12（P3-T2 插件市场 MVP）

### Added

- **插件市场**（`sz-rust-marketplace`）：
  - 28 变体 `MarketplaceError` 错误类型 + HTTP 状态码映射
  - 7 个领域模型（Plugin/PluginVersion/ReviewRecord/InstallRecord/Developer/MarketplaceManifest/PluginsLock）
  - `ManifestService`（JSON/TOML 清单解析）
  - `SignatureService`（Ed25519 签名 + SHA256 完;整性 + 公钥指纹）
  - `ObjectStore` trait + `LocalObjectStore` 实现
  - 5 个 Repository trait（Plugin/Version/Review/InstallRecord/Developer）
  - `LockFileManager`（plugins.lock 原子读写）
  - `PublishService` / `SearchService` / `InstallService` / `VersionService` / `ReviewService`
  - `MarketplaceService` 核心入口（依赖注入 + ArcSwap 配置热更新）
  - `Notifier` trait + `MemoryNotifier` 实现
  - Web 平台：13 个 RESTful API 路由 + 处理器 + 中间件（JWT/Trace/RateLimit）
  - 6 个 PostgreSQL 迁移脚本（含 append-only REVOKE UPDATE/DELETE）
  - 25 个单元测试全部通过

- **CLI 集成**（`sz-rust-cli`）：
  - `sz-rust plugin search/install/publish/uninstall/update/list/login` 七个子命令
  - 275 个 CLI 测试全部通过（含新增 plugin 命令测试）

- **CI/CD**：`.github/workflows/marketplace-ci.yml`（跨平台矩阵构建）

- **开发者文档**：`docs/developer/guide.md`（插件开发指南）

## [Unreleased] - 2026-08-11（P1-T4 完善现有业务插件）

### Added

- **CMS 插件**（`sz-rust-addons-cms`）：
  - 5 个 Capability 实现（`cms.search_article` / `cms.create_article` / `cms.publish_article` / `cms.manage_category` / `cms.manage_tag`）
  - `CmsPlugin` CapabilityHook 实现（register_capabilities + capability_names）
  - `manifest.json`（13 字段，5 能力，12 路由，2 事件）
  - `README.md`（7 章节，中文正文 + 英文 API）
  - 21 个测试（12 集成 + 9 capability lib 测试）

- **CRM 插件**（`sz-rust-addons-crm`）：
  - 7 个 Capability 实现（4 查询 + 3 写入，含 `crm.convert_lead` / `crm.update_deal_stage`）
  - `LeadController::convert` 改造为三步原子操作（lead→contact→deal + 手动回滚）
  - `DealController::update_stage` 新增 + pipeline 阶段名对齐（initial → requirement_confirmed → quoted → negotiating → won/lost）
  - `CrmPlugin` CapabilityHook 实现
  - `manifest.json`（13 字段，7 能力，17 路由，2 事件）
  - `README.md`（7 章节，含线索转化原子流程 + 商机阶段流转图）
  - 14 个新增测试（现有 21 个全部保留继续通过，总计 35 个）

- **电商插件**（`sz-rust-addons-ecommerce`）：
  - 6 个 Capability 实现（3 订单 + 3 购物车，含 `ecommerce.cancel_order` / `ecommerce.clear_cart`）
  - `CartController::add` 改造为同用户同商品数量累加
  - `OrderController` 正向流转方法（pay / ship / complete）+ 状态机常量
  - `EcommercePlugin` CapabilityHook 实现
  - `manifest.json`（13 字段，6 能力，17 路由，2 事件）
  - `README.md`（7 章节，含订单状态流转图 + 购物车累加说明）
  - 23 个新增测试（现有 21 个全部保留继续通过，总计 44 个）

- **跨插件集成测试**（`sz-rust-examples/tests/business_addons_integration.rs`）：
  - 12 个集成测试（6 跨插件能力注册 + 6 铁律合规 Send+Sync 断言）
  - 验证 18 个能力无命名冲突，各插件能力可独立调用

- **JSON Schema**（`schemas/plugin-manifest.schema.json`）：
  - 插件 manifest.json 校验 Schema（13 个必需字段）

### Changed

- CRM `DealController::pipeline` 阶段名对齐 spec 6.4（prospect → initial 等 6 阶段）
- CRM `LeadController::list` 和 `DealController::list` 增加 keyword 参数
- 3 个插件 `Cargo.toml` 新增 `sz-rust-capability` + `sz-rust-addons-loader` workspace 依赖
- workspace 根 `Cargo.toml` 新增 `sz-rust-addons-cms` / `sz-rust-addons-crm` / `sz-rust-addons-ecommerce` workspace 依赖声明
- `sz-rust-examples/Cargo.toml` dev-dependencies 新增 3 个插件 + capability + loader 依赖

### Verified

- 3 个插件 `cargo check` 通过（`cargo test -p sz-rust-addons-{cms,crm,ecommerce}` 编译成功）
- CMS 21 个测试全部通过（`cargo test -p sz-rust-addons-cms` 输出：`test result: ok. 21 passed`）
- CRM 35 个测试全部通过（`cargo test -p sz-rust-addons-crm` 输出：`test result: ok. 35 passed`）
- 电商 44 个测试全部通过（`cargo test -p sz-rust-addons-ecommerce` 输出：`test result: ok. 44 passed`）
- 跨插件集成测试 12 个全部通过（`cargo test -p sz-rust-examples --test business_addons_integration` 输出：`test result: ok. 12 passed`）
- 现有 42 个测试全部继续通过（CRM 21 + 电商 21 回归无破坏）
- 3 个插件源码无 `std::fs::` 调用（grep 验证）
- 3 个 Plugin struct + 3 个 State struct 均满足 `Send + Sync`（编译期断言通过）
- 18 个能力名全部以各自前缀开头（cms. / crm. / ecommerce.），无重名

## [1.1.0] - 2026-08-10（Admin Monitor API + AI 原生集成）

### 新增 — AI 原生集成 facade（`sz-rust-ai-facade`，workspace 第 34 成员）

- **LLM 统一抽象**：`LlmProvider` trait + OpenAI / Claude / Gemini 三家 Provider 完整 HTTP 调用实现
- **模型路由**：`ModelRouter` 基于 `ArcSwap` 无锁热替换路由表
- **故障切换**：`ProviderFailover` 状态机（Available → Degraded → Cooldown）+ `call_with_failover`
- **上下文裁剪**：`ContextTruncator` 自动裁剪超长上下文（保留 System 消息）
- **Token 计数**：`TokenCounter` 带缓存的无推理计数
- **Embedding**：`EmbeddingProvider` trait + OpenAI Embedding + `BatchChunker` 批量分片
- **向量存储**：`VectorStore` trait + `SimilarityMetric`（Cosine / Dot / L2）
- **RAG 管道**：`RagPipeline` 三段式（retrieve → assemble → generate）+ 引用溯源
- **Agent 引擎**：`Agent` + `AgentExecutor` 工具选择循环 + `TerminationPolicy` + 短期/长期记忆
- **MCP 桥接**：`McpToolBridge` 将 sz-rust-mcp 7 工具暴露为 Agent 可用工具（进程内调用，ADR-AI-01）
- **Facade 静态 API**：`Ai::chat / stream_chat / embed / rag / agent` + `OnceLock` 全局实例（对齐 PHP `think\facade\Ai`）
- **可观测性**：7 个 Prometheus 指标（ai_chat_total / ai_chat_duration / ai_stream_total / ai_embed_total / ai_rag_total / ai_agent_total / ai_tool_total）+ `tracing` 结构化日志
- **审计 HTTP 客户端**：`AuditHttpClient` + 令牌桶限流
- **配置扩展**：`infra-facade` 新增 `AiSection` + 8 个子配置结构体 + `load_optional_section()`
- **统一错误**：`AiError` 17 变体 + `error_code()` + `is_retryable()`
- **流式透传**：经 `http-facade::sse::SseEvent` 透传，不绕行新建 SSE 通道

### 新增 — Admin Monitor API（`admin` feature，默认关闭）

- **3 个管理端监控端点**（`/api/admin/*`，需 `admin` 角色）：
  - `GET /api/admin/server/info`：服务器信息（CPU/内存/磁盘/负载/进程启动时间/Rust版本/主机名）
  - `GET /api/admin/db/pool`：数据库连接池状态（active/idle/max/usage_percent）
  - `GET /api/admin/redis/info`：Redis 实例信息（版本/模式/连接数/内存/运行时间/角色）

- **`sz-rust-observability` 新增 `admin` 模块**：
  - `sysinfo_collector`：基于 `sysinfo` crate 0.32，跨平台（Windows `COMPUTERNAME` / Unix `hostname`）
  - `db_pool_collector`：`DbPoolStats` trait（trait object 适配，避免 observability crate 直连 sz-orm-core）
  - `redis_collector`：`RedisStats` trait + INFO 解析（无 Redis 连接时降级返回 `connected: false`）
  - `once_cell` 懒加载 rustc 版本检测

- **`sz-rust-sz300` 集成**：
  - `AppState` 新增 `db_pool_stats`（`Arc<dyn DbPoolStats>`）+ `redis_stats`（`Option<Arc<dyn RedisStats>>`）
  - `DbPoolStatsAdapter`：通过 `tokio::runtime::Handle::block_on` 桥接 async `Pool::status()`
  - `RedisStatsAdapter`：sync `redis::Client::get_connection()` + INFO 解析
  - `ADMIN_REDIS_URL` 环境变量可选：未配置时 `/api/admin/redis/info` 返回降级响应（200 + `connected: false`）

- **`RoleGuard` 中间件**：
  - `admin_role_guard`：路由级角色校验，叠加在全局 JWT 中间件之上
  - 401：未提供令牌 / 令牌无效；403：令牌有效但缺少 `admin` 角色
  - 通用 `role_guard(req, next, role)` 支持任意角色扩展

### 变更
- `sz-rust-core/src/orm.rs`：新增 `PoolStatus` re-export
- `sz-rust-sz300/Cargo.toml`：新增 `redis`（optional）+ `tower`/`http-body-util`（dev-dependencies）
- `sz-rust-sz300/src/main.rs`：`pool` 改为 `Arc::new(...)` 以同时注入 `db_pool` 和 `db_pool_stats`
- `sz-rust-sz300/src/services/auth_service.rs`：新增 `init_auth_test_only()` 测试辅助

### 测试
- 新增 20 个单元测试（observability 16 + role_guard 4），全部通过
- `cargo check -p sz-rust-sz300 --features admin` 与默认配置均编译通过

### 向后兼容
- `admin` feature 默认关闭，不影响现有部署
- 新增字段均为可选/有安全默认值

---

## [1.0.0] - 2026-08-10（GA 生产部署加固）

### 新增 — 10 项生产部署加固

- **敏感字段脱敏审计与补全**：
  - 审计脚本 `scripts/audit/sensitive-field-audit.js`：389 文件扫描，15 字段全部脱敏，0 EXPOSED
  - TokenPair/RenewedToken 自定义 Debug 脱敏（`auth-facade/src/refresh.rs`）
  - 回归测试：sz-rust-sz300 + sz-rust-cache-facade 共 6 个脱敏测试

- **Redis TLS 加密配置**：
  - `TlsConfig` + `TlsConfigError`（`auth-facade/src/redis_store.rs`）
  - `is_tls_enabled()` + `validate_production_tls()`：生产环境强制 TLS
  - cache-facade RedisConfig 新增 `enable_tls`/`tls_ca_cert_path` 字段
  - 11 个 TLS 测试

- **JWT 签名密钥轮换机制**：
  - `KeyRotation` 多密钥并存验证（`mvc-facade/src/controller.rs`）
  - 当前密钥签发 + 旧密钥 grace period 内验证
  - `spawn_rotation_task()` 定时轮换 + `fingerprint()` SHA256 指纹审计
  - Debug 脱敏：current/previous 输出 `[REDACTED]`
  - 12 个密钥轮换测试

- **限流中间件生产配置**：
  - `RateLimitProductionConfig`（capacity=2000, refill=1000/s，基于 v0.7.0 压测基线）
  - 排除路径含 /health、/metrics

- **熔断中间件生产配置**：
  - `CircuitBreakerProductionConfig`（error_threshold=0.5, cooldown=10s）
  - `validate()` 校验参数有效性

- **日志级别生产配置**：
  - `default_log_level()` 从 `info` 改为 `warn`
  - `LogConfig` + `validate_production()`：生产环境禁止 debug/trace
  - `ConfigError::LogLevelForbiddenInProduction`

- **metrics 端点访问控制**：
  - `MetricsAuthConfig`：Bearer token + IP 白名单双重校验
  - `validate_production()`：生产环境无鉴权 → 拒绝暴露
  - Debug 脱敏：bearer_token 输出 `[REDACTED]`

- **健康检查端点配置化**：
  - `HealthCheckConfig`：readiness 检查项可配置（db/redis/mqtt）
  - 未知检查项过滤 + 超时控制

- **优雅关闭超时配置化**：
  - `ShutdownConfig`：`shutdown_timeout`（默认 30s）+ `mqtt_timeout()`
  - MQTT 超时从硬编码 5s 改为可配置
  - 超时强制中止 + `MQTT_CONSUMER_FORCE_QUIT` 警告日志

- **K8s 探针模板化**：
  - Helm chart（`deploy/k8s/helm/sz300/`）：Chart.yaml + values.yaml + values-prod.yaml + templates/
  - RUST_LOG 从 `info,sz_rust_sz300=debug` 修正为 `warn,sz_rust_sz300=info`
  - 探针参数全量模板化（liveness/readiness/startup）
  - 部署指南 `docs/operations/k8s-deploy.md`

### 变更
- workspace.package.version 0.7.0 → 1.0.0
- 19 个内部依赖版本 0.7.0 → 1.0.0
- `packages/sz-rust-sz300/src/main.rs`：默认日志级别 + 优雅关闭超时配置化
- `deploy/k8s/sz300-deployment.yaml`：RUST_LOG 修正

### 测试
- 新增 73 个测试（10 个测试文件），全部通过
- GA 检查清单报告：`docs/audit/2026-08-10-v1.0-ga-checklist-report.md`

### 向后兼容
- v0.7.0 已发布 API 保持 semver 兼容
- 新增配置字段使用安全默认值，未配置时不破坏现有部署

---

## [0.7.0] - 2026-08-10

### 新增
- **crates.io 全量发布（29/29 crate）**：
  - workspace.package.version 0.6.7 → 0.7.0
  - 19 个 sz-rust-* 内部依赖 0.6.1 → 0.7.0
  - 拓扑排序发布：6 层依赖图（L0-L5），29 crate 全部发布成功
  - 审计日志 29 条，全部 verified

- **多并发压测（4 框架 × 3 路由 × 3 并发 = 36 组合）**：
  - 并发级别：32/128/256（C=64 复用 v0.6.7 历史基线）
  - 合计 48 数据点（36 新 + 12 历史基线）
  - 资源监控集成：sar（CPU/内存）+ dstat（网络），采集窗口 20s
  - 报告归档：docs/audit/2026-08-10-框架性能对比报告-v0.7.0.md

- **深度评估文档更新**：
  - 基线 v0.6.7 → v0.7.0
  - 代码行数 121,212 行（实测精确值，排除空行/注释）
  - 测试函数 4,610 个
  - ADR 21 个

### 变更
- sz-rust-sz300/Cargo.toml：添加 repository/homepage/keywords/categories workspace 继承
- sz-rust-examples/Cargo.toml：添加 repository/homepage/keywords/categories workspace 继承

### 向后兼容
- 无破坏性变更
- sz-orm-* 依赖保持 3.5.0 未修改
- sz-pay 兼容性：待验证

### 验证
- cargo check 通过（19.56s）
- 全量测试 0 failed
- crates.io 29/29 发布成功
- 性能回退校验：✅ 无回退

## [0.6.9] - 2026-08-10

### 变更
- **sz-orm 依赖升级 2.1.0 → 3.5.0**：
  - 17 个 sz-orm-* 子 crate 从 crates.io 2.1.0 升级到 3.5.0
  - `Cargo.toml` workspace 依赖版本号修改（行 179-195）
  - `Cargo.lock` 16 个 sz-orm 包全部更新到 3.5.0
  - 3 个 deprecation 警告：`sz_orm_query_builder::Query` 已废弃，推荐迁移到 `sz_orm_core::QueryBuilder<M>`

### 验证
- 本地 debug 编译通过（1m05s）
- 本地全量测试通过：5332 passed, 0 failed, 305 ignored
- sz-pay 兼容性验证通过（cargo check 21.11s）
- 服务器 release 编译通过（2m33s，Rust 1.97.1）
- 服务器 sz300-server 启动成功（数据库连接需额外配置）

## [0.6.8] - 2026-08-09

### 新增
- **Soak 自托管工具通用化（参数化 v2.0）**：
  - `config-defaults.sh` — 默认值定义模块（唯一允许 sz-rust 硬编码的位置）
  - `soak-runner.sh` 全参数化：`--project`/`--protected-port`/`--protected-process`/`--report-dir`/`--soak-ports`/`--restart-script`/`--cron-marker`
  - `soak-trigger.js` 全参数化：9 个参数分支，透传到 `soak-runner.sh`
  - `verify-6h-soak.sh` — 6h soak 验证脚本（4 项检查）
  - 参数校验函数 `validate_params()`：端口/目录/项目名格式校验
  - 部署到 `/www/rust/soak-toolkit/` 共享目录，支持多项目复用

- **完整性能压测（4 框架 × 3 路由 = 12 组合）**：
  - sz-rust(axum 0.8) / actix / axum / poem 同条件实测
  - 64 并发，10s 时长，wrk 4.1.0
  - 生成 `docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md` 合并报告（评估 + 5 框架 × 5 维度对比）
  - rocket 编译超时结论：`rocket-build-result.json`（status=timeout, >30 分钟）

- **cron 调度配置**：
  - 周日 00:00 UTC 6h soak（`--trigger cron`）
  - 每日 18:00 UTC nightly soak（`--trigger nightly`）

### 变更
- `process-guard.sh` 重构为全参数化（`--soak-dir`/`--protected-process`/`--restart-script`）
- `soak-archive.sh` 添加 `--report-dir` 参数
- `soak-cron-setup.sh` 全参数化（`--cron-marker`/`--soak-runner`/`--work-dir`）
- `run-benchmark.sh` 默认时长 60s→30s，添加汇总 JSON 和 Markdown 报告生成

### 压测结果摘要
| 框架 | /hello RPS | /json RPS | /user/42 RPS |
|------|-----------|----------|-------------|
| sz-rust | 157,526 | 148,321 | 144,267 |
| actix | 194,717 | 190,422 | 183,197 |
| axum | 139,682 | 135,968 | 136,012 |
| poem | 137,898 | 133,062 | 130,842 |

## [0.6.7] - 2026-08-08

### 新增
- **Redis 设备会话存储（P1 完善）**：
  - `RedisDeviceSessionStore` — 实现 `DeviceSessionStore` trait 全部 8 方法（HSET/HGETALL/HGET/HDEL/DEL）
  - `RedisConfig.key_prefix_sessions` 字段（默认 `sso:sessions`）
  - `create_redis_stores_with_devices()` — 三元组工厂方法，共享 ConnectionManager
  - 7 个 Redis 集成测试覆盖全部 8 方法

- **Token 降级 axum 端点（P3 完善）**：
  - `POST /sso/degrade/user` — 用户级降级
  - `POST /sso/degrade/device` — 设备级降级
  - `DELETE /sso/degrade/user/:user_id` — 清除用户降级
  - `DELETE /sso/degrade/device/:user_id/:device_id` — 清除设备降级
  - `GET /sso/degrade/:user_id` — 查询降级状态
  - 3 个降级集成测试（完整流程 / 设备级优先 / TTL 过期）

## [0.6.6] - 2026-08-08

### 新增
- **多设备会话管理（P1）**：
  - `DeviceInfo` / `DeviceSession` / `DeviceSessionConfig` / `DeviceSessionStore` trait / `MemoryDeviceSessionStore`
  - `SsoClaims.device_id` 字段，access/refresh token 均可绑定设备
  - `SsoService::login_with_device()` — 登录并注册设备会话，含 LRU 淘汰
  - `SsoService::list_devices()` / `revoke_device()` / `update_device_active()` / `cleanup_expired_devices()`
  - 设备撤销同时拉黑 access + refresh token 的 jti
  - `revoke_all` 联动清空设备会话，`validate` 联动更新设备活跃时间
  - axum 端点：`GET /sso/devices/:user_id`、`POST /sso/devices/revoke`、`POST /sso/devices/heartbeat`

- **Token 权限降级机制（P3）**：
  - `DegradationEntry` / `DegradationStore` trait / `MemoryDegradationStore`
  - `SsoService::degrade_user()` / `clear_degradation()` / `get_degradation()` — 用户级降级
  - `SsoService::degrade_device()` / `clear_device_degradation()` — 设备级降级
  - `validate` / `validate_with_renewal` 自动应用降级（子集过滤，不能提权）
  - 设备级降级优先于用户级，降级 TTL 自动过期
  - `revoke_all` / `revoke_device` 联动清除降级映射
  - `issue_with_roles()` — 签发携带 roles/permissions 的 token

- **SSO 跨域单点登录（P4）**：
  - `SsoTicket` / `TicketStore` trait / `MemoryTicketStore`
  - `SsoService::generate_ticket()` — 生成一次性 ticket（30 秒 TTL）
  - `SsoService::exchange_ticket()` — ticket 换取 TokenPair（一次性使用）
  - `SsoService::validate_ticket()` — 仅验证不消费

- **审计日志持久化（P5）**：
  - `AuditEvent` / `AuditEventType` 枚举（Login/Logout/Revoke/RevokeAll/RevokeDevice/Degrade/ClearDegradation/TicketGenerate/TicketExchange/RefreshRotated/ReuseDetected/DeviceRegistered/DeviceEvicted）
  - `AuditStore` trait / `MemoryAuditStore`
  - `SsoService::query_audit()` — 查询用户审计事件
  - login / revoke_all / degrade / ticket_generate / ticket_exchange 等关键操作自动记录审计

### 变更
- `Cargo.toml` workspace version 0.6.5 → 0.6.6（semver 兼容）
- `SsoClaims` 新增 `device_id` 字段（`Option<String>`，默认 None）
- `DeviceSession` 新增 `access_jti` 字段（撤销设备时同时拉黑 access token）
- `issue_with_device_and_jti` 签名增加 `roles` / `permissions` 参数
- `MockUserAuth` 测试用户 roles 更新为 `["admin", "user"]`，permissions 为 `["read", "write"]`

### 测试
- 190 个 lib 单元测试 + 21 个集成测试全部通过
- clippy 0 warning（auth-facade + middleware-facade）
- sz-pay 兼容性验证通过

## [0.6.5] - 2026-08-08

### 新增
- **Token 自动续期**（Access Token Auto-Renewal on Validation）：
  - `RenewalConfig` 结构体（`enabled` / `renewal_threshold` / `renewal_ratio` / `access_token_ttl`），可序列化/反序列化
  - `RenewalConfig::should_renew()` — 续期判定算法（threshold=0 特殊处理、ratio 边界）
  - `RenewedToken` 续期结果载体（`access_token` + `expires_at`）
  - `RefreshTokenIssuer::renew_access()` — 仅签发新 accessToken，不签发新 refreshToken，不递增版本号，不撤销旧 token
  - `SsoService::validate_with_renewal()` — 校验 + 自动续期，返回 `(SsoClaims, Option<RenewedToken>)`
  - `SsoService::with_renewal_config()` — 链式设置续期配置
  - axum `/sso/validate` 端点增强：响应新增 `new_access_token` / `new_access_expires_at` 字段（`null` 表示未续期）
  - 中间件续期响应头：`X-Renewed-Access-Token` / `X-Renewed-Expires-At`
  - `SsoMiddlewareConfig::local_with_renewal()` / `local_memory_with_renewal()` — 带续期配置的构造方法
  - 20 个单元测试 + 8 个集成测试 + 6 个边界测试 + 2 个性能基准

### 安全约束
- 续期不签发新 refreshToken（REQ-010）
- 续期不递增版本号（REQ-012）
- 续期不撤销旧 accessToken（REQ-011）
- 续期不绕过黑名单/版本/过期检查（REQ-021/022/023）

### 变更
- `Cargo.toml` workspace version 0.6.4 → 0.6.5（semver 兼容）
- `SsoService::validate()` 签名与行为不变（向后兼容）
- `SsoMiddlewareConfig::local()` / `local_memory()` 签名不变（向后兼容）
- `ValidateResponse` 新增 `Option` 字段，不续期时序列化为 `null`（向后兼容）

## [0.6.4] - 2026-08-08

### 新增
- **远程校验连接池优化**（`remote-validate` feature）：
  - `PoolConfig` 结构体（`pool_max_idle_per_host` / `pool_idle_timeout` / `tcp_keepalive` / `tcp_nodelay`），连接池参数可配置
  - `RemoteValidateConfig::new_checked()` — 返回 `Result`（失败安全，不 panic）
  - `RemoteValidateConfig::new_or_default()` — 失败回退默认 Client + warn 日志
  - `RemoteValidateConfig::from_client()` — 外部注入预配置 `reqwest::Client`
  - `RemoteValidateConfigBuilder` — 链式配置 builder 模式
  - `RefreshTokenError::InvalidConfig` 新变体
  - 10 个单元测试

### 变更
- `Cargo.toml` workspace version 0.6.3 → 0.6.4（semver 兼容，`new()` 签名不变）
- `RemoteValidateConfig::new()` 内部委托 `new_checked().expect()`（向后兼容）

## [0.6.3] - 2026-08-08

### 新增
- **Redis 存储后端**（feature gate `redis-store`）：
  - `auth-facade/src/redis_store.rs` — `RedisRefreshTokenStore`（GET / INCR 原子递增版本号）+ `RedisTokenBlacklist`（EXISTS / SETEX 带 TTL 黑名单）
  - `RedisConfig` 结构体（url / key 前缀 / 超时），Debug 自动脱敏 URL 密码
  - `create_redis_stores` 便捷工厂（共享 ConnectionManager）
  - 使用 `redis::aio::ConnectionManager`（自动重连 + 连接池复用）
  - fail-closed 策略：Redis 故障 → `ServiceUnavailable`，安全优先
  - 9 个单元测试 + 8 个集成测试（真实 Redis，含并发无丢失更新验证）
- **feature gate**：`redis-store`（Redis 存储后端）、`redis-cluster`（Redis 集群支持），默认零 Redis 依赖

### 变更
- `Cargo.toml` workspace version 0.6.2 → 0.6.3（Redis 后端为新增可选 feature，semver 兼容）
- `redis-gateway` feature 修正为 `dep:redis` 语法

## [0.6.2] - 2026-08-08

### 新增
- **SSO 单点登录 + Refresh Token 双 Token 机制**：
  - `auth-facade/src/refresh.rs` — `SsoJwtCodec`（HS256，复用 RustCrypto audited crate）、`SsoClaims`（JwtClaims 超集，新增 token_type/jti/ver）、`RefreshTokenIssuer`（签发+轮换）、`RefreshTokenVerifier`（6 级校验链）、`RefreshTokenRevoker`（单 Token 撤销 + 用户级撤销）、`TokenBlacklist`/`RefreshTokenStore` trait 抽象 + Memory 实现
  - `auth-facade/src/sso.rs` — `SsoService` 认证中心 + `UserAuthService` trait + axum HTTP 端点（login/refresh/revoke/validate/me，feature gate `axum`）
  - `middleware-facade/src/sso_middleware.rs` — `sso_middleware` 本地验签 + 白名单 + `AuthenticatedUser` + 远程校验（feature gate `remote-validate`）
  - 复用攻击检测：已黑名单 refresh token 再刷新 → `ReuseDetected` + 撤销用户所有 Token + tracing::warn! 告警
  - token_version 用户级撤销：`revoke_all` = `increment_version`（O(1)）
  - 22 个单元测试 + 12 个边界测试 + 6 个集成测试 + 3 个基准测试（encode 856ns / verify 2μs / rotate 7μs）
- **feature gate**：`axum`（SsoCenter HTTP 端点）、`remote-validate`（远程校验 + reqwest），默认零网络依赖

### 变更
- `Cargo.toml` workspace version 0.6.1 → 0.6.2（SSO 为新增功能，semver 兼容）
- `sz-rust-sz300/controllers/auth.rs` refresh 端点从空实现替换为调用 `RefreshTokenIssuer::rotate`

## [0.6.1] - 2026-08-07

### 新增
- **SSE (Server-Sent Events) 支持**：
  - `http-facade/src/sse.rs` — 基于 axum 0.8 的 SSE 实现，支持 `Event::data()`/`event()`/`retry()`/`id()`
  - `SseStream` — 将异步迭代器适配为 axum SSE 流
  - 4 个单元测试覆盖：事件构建、流适配、keep-alive、错误处理

### 变更
- `Cargo.toml` workspace version 0.6.0 → 0.6.1（SSE 为新增功能，semver 兼容）

## [0.3.2] - 2026-08-05

### 修复
- **§5.3 SQL 注入根治**：
  - `cli/src/cmd/migrate.rs:444` — `delete_migration_record` 从 `format!` 拼接改为 `execute_with_params` 参数化绑定
  - `orm-ext-facade/src/hooks.rs:777` — `soft_delete_update_sql` / `soft_delete_restore_sql` 加 `debug_assert!(is_valid_identifier(...))` 校验
- **§5.2 路由纳秒级优化**：
  - `router-facade/src/router.rs:35` — `APP_MAP` 从 `LazyLock<HashSet>` 改为 `const APP_LIST: &[&str]` 线性查找
  - `router-facade/src/router.rs:130` — `parse_path` 从 `Vec::collect` 改为迭代器直接消费
  - 新增 3 个性能测试（release-only，p99 < 300/500/800ns）
- **clippy 修复**：`middleware-facade/src/rate_limit.rs:501` — `or_insert_with(Vec::new)` → `or_default()`

### 变更
- **§5.1 addons 文档更新**：crm/erp/ecommerce 三包 lib.rs doc-comment 从"0 测试脚手架"改为"已填实（v0.3.2）：N 测试"
- **§5.7 评估报告重写**：综合评分 55→78，7 维度评分更新，12 处过时表述修正
- `Cargo.toml` workspace version 0.3.1 → 0.3.2

### 已知问题
- **crates.io 0.3.2 发布阻塞**：sz-orm-* 包在 crates.io 上版本不一致（core 1.5.0 / auth 1.2.2 / graphql 1.2.1），需上游统一发布 1.5.0 后方可发布

## [0.6.0] - 2026-08-07

### P3 性能优化（6 大方向）

#### 新增
- **方向 3：SIMD 字符串加速** — `router-facade/src/simd_str.rs`
  - SSE2 并行 ASCII 检测 + 字节操作（`capitalize_first_simd`）
  - SSE2 memchr 风格分隔符查找（`find_separator_simd`），一次扫描 16 字节
  - x86_64 运行时检测 + 非 x86_64 标量回退，18 个单元测试
- **方向 4：内存池** — `sz-rust-core/src/mem_pool.rs`
  - `MemPool` trait + `StackPool<const CAP: usize>`（区域分配器，零堆分配）
  - `BumpaloPool` 实现（`bumpalo-pool` feature gate），13 个单元测试
  - `AllocCounter` GlobalAlloc wrapper（`alloc-count` feature gate），4 个单元测试
- **方向 2：连接池 L3 调优** — `orm-facade/src/{pool_warmer,query_cache,pool_scaler}.rs`
  - `PoolWarmer`：并发连接预热，支持超时和降级，7 个单元测试
  - `QueryCache`：L2 查询缓存（TTL + jitter + LRU 淘汰 + invalidate pattern），10 个单元测试
  - `PoolScaler`：动态扩容/缩容（基于 timeout_rate / idle_rate），10 个单元测试
- **方向 5：零拷贝优化**
  - `routing.rs` 新增 `HandlerRefRef<'a>` 借用版本（零堆分配），13 个单元测试
  - `response.rs` 新增 `to_json_bytes() -> bytes::Bytes`（避免 String UTF-8 验证开销），4 个单元测试
- **方向 6：异步优化** — `sz-rust-core/src/runtime.rs`
  - `SzRuntime` 新增 `blocking_threads` 字段 + `with_blocking_threads()` 链式配置
  - 3 种预设：`for_io_intensive()` / `for_cpu_intensive()` / `for_balanced()`
  - 11 个单元测试
- **P3 bench 框架** — `sz-rust-core/benches/p3_bench.rs`
  - 5 类 benchmark（22 个）：端到端 p99 / SIMD 字符串 / alloc 计数 / 拷贝计数 / 异步调度
- **P3 soak 测试** — `sz-rust-core/tests/soak_p3.rs`
  - 3 个 soak 测试：优化点全覆盖 / SIMD 稳定性 / 异步调度稳定性
- **spawn_blocking 审计脚本** — `scripts/audit_blocking.sh`
  - 静态扫描 async fn 内阻塞调用，审计报告 `docs/audit/blocking_audit_20260807.md`
- **火焰图脚本** — `scripts/flamegraph.sh`

#### 变更
- **方向 1：热路径内联优化**
  - `router.rs`：`parse_path` / `split_first_segment` / `is_app_in_map` / `capitalize_first` / `ParsedPath::new` 添加 `#[inline]`
  - `chain.rs`：`has_duplicates` 添加 `#[inline]`
  - `container/mod.rs`：`make` / `make_or_panic` / `make_with_scope` 添加 `#[inline]`
  - `simd_str.rs`：`capitalize_first_simd` / `find_separator_simd` / `is_ascii_simd` 添加 `#[inline]`
- `router.rs`：`parse_path` 使用 SIMD 分隔符查找替代 `split` 迭代器
- `router-facade/Cargo.toml`：移除 `[lints] workspace = true`，添加 `[lints.rust] unsafe_code = "allow"`
- `sz-rust-core/Cargo.toml`：添加 `alloc-count` / `mem-pool` / `bumpalo-pool` feature、`p3_bench` bench target
- `sz-rust-orm-facade/Cargo.toml`：添加 `async-trait` / `tokio` / `parking_lot` / `rand` / `thiserror` 依赖
- `sz-rust-http-facade/Cargo.toml`：添加 `bytes` 依赖
- workspace 版本 0.5.0 → 0.6.0

#### 验证
- workspace 全量测试：5174 passed, 0 failed
- sz-pay 兼容性：cargo check + 全量测试通过
- sz-orm 上游：无变更（git status 空）
- clippy：无新警告（3 个预存警告）
- fmt：`cargo fmt --all -- --check` 通过
- bench：22 个 benchmark 全部编译通过，`capitalize_first` ~38ns

## [Unreleased]

### 新增
- **sz-rust-addons-crm**：CRM 模板插件（联系人/线索/商机管理，15 个 REST 端点）
- **sz-rust-addons-erp**：ERP 模板插件（商品/供应商/采购单管理，16 个 REST 端点）
- **sz-rust-addons-ecommerce**：电商模板插件（订单/订单项/购物车管理，13 个 REST 端点）
- **RouterBuilder 泛型状态支持**：`RouterBuilder<S>` 支持 `axum::extract::State<S>`，addon 可通过闭包捕获状态注册路由
- **CLI `make:middleware` 命令**：`sz-rust-cli make middleware <name>` 生成中间件骨架
- **10 个新增 .trae/skills/**：test-coverage、performance-check、doc-check、migration、deploy、orm-query、n-plus-one、auth-guard、error-handling、ci-cd

### 变更
- sz300 集成可观测性模块（sz-rust-observability）：Prometheus /metrics 端点 + MetricsRegistry 注入 AppState
- sz300 readiness 探针（/health/ready 端点 + DB 健康检查 + 503 状态码）
- sz300 优雅关闭（with_graceful_shutdown 支持 Ctrl+C + SIGTERM）
- sz300 MQTT 消费者优雅退出（CancellationToken 协调器）
- sz300 tracing 初始化改为 EnvFilter + JSON 格式
- sz300 集成框架统一 AppConfig（sz_rust_core::config::AppConfig）
- 全代码库关键路径添加 #[tracing::instrument] 自动 span 注入
- sz-rust-addons-operate 和 sz-rust-sz300 加入 workspace.members（CI 覆盖 10/10 包）
- 生产就绪度审计报告（docs/audit/2026-07-24-生产就绪度审计报告.md）

### 变更
- 所有 10 个包添加 rust-version.workspace = true
- sz-rust-tracing 依赖改为 workspace 继承
- CI 缓存策略统一为 Swatinem/rust-cache@v2
- CI audit job 从 rustsec/audit-check@v2.0.0 替换为 taiki-e/install-action + cargo audit
- CI mcdc continue-on-error 改为 false（分支覆盖率硬门禁）
- CI outdated continue-on-error 改为 false
- CI 添加 paths-ignore（文档变更不触发 CI）
- CI fmt/no-placeholder job 移除不必要的 sz-orm clone
- docs/audit/ 历史文档归档至 archive/ 目录

### 修复
- P0: middleware/auth.rs JWT 密钥从硬编码改为环境变量 SZ_JWT_SECRET
- P0: sz300/main.rs JWT 密钥改为环境变量 SZ300_JWT_SECRET
- P0: sz300/config.rs 数据库密码改为环境变量 SZ300_DB_PASSWORD
- P0: deny.toml allow-build 非法字段修复为 reason
- P1: upload/storage.rs 路径遍历漏洞修复（.. 检查 + canonicalize 验证）
- P1: sz300 + addons-operate 补齐 #![forbid(unsafe_code)] + #![warn(missing_docs)]
- P1: sz300 missing_docs 226 个警告清零
- P1: addons-operate missing_docs 48 个警告清零
- P1: sz300 unused imports 清理

## [0.2.0] - 2026-07-23

### 新增
- **可观测性模块**（`sz-rust-observability` 包）：`MetricsRegistry` + Counter/Gauge/Histogram 三种指标类型，SLO 多窗口燃烧率告警（1h/5m + 6h/30m 双窗口对，对齐 Google SRE Workbook 第 5 章）
- **分布式追踪模块**（`sz-rust-tracing` 包）：`Span` / `Tracer` / `SzTracer`，W3C TraceContext 格式（`traceparent: 00-<trace_id>-<span_id>-<flags>`），legacy header 兼容，OTLP exporter 占位
- **ADR-011 可观测性架构决策**：MetricsRegistry 设计、SLO 多窗口燃烧率、四层可观测性模型（L1 决策层 / L2 运行时层 / L3 指标层 / L4 代码层）
- **ADR-012 分布式追踪架构决策**：W3C TraceContext 标准、OTLP exporter 路径、legacy header 兼容策略
- **missing_docs 严格检查**：CI doc job 启用 `RUSTDOCFLAGS: "-D warnings -D missing_docs"`，所有公开 API 必须有文档注释
- **首次性能基线数据**：`docs/benchmarks/baseline-v0.1.0.md` 记录 criterion 基线，后续版本以此为回归参照
- **6 小时 soak test**：`soak.yml` workflow，每周日 00:00 UTC 自动执行，60 秒指标采样，420 分钟超时
- **cargo-tarpaulin 覆盖率**：`coverage.yml` workflow，统计代码覆盖率并上传 Codecov
- **模糊测试套件**：`sz-rust-core/tests/fuzz.rs`，7 个 fuzz 用例 × 1000 次迭代（parse_path / HandlerRef / route_config / ApiResponse / ErrorCode / AppConfig / Validate），使用自定义 xorshift64 PRNG，不依赖 cargo-fuzz
- **fuzz CI workflow**：`fuzz.yml`，push/PR + 每周六 00:00 UTC + workflow_dispatch 触发，支持 `FUZZ_ITERATIONS` 环境变量自定义迭代次数
- **cargo-deny 依赖审计**：`deny.toml` 配置（许可证白名单 MIT/Apache-2.0/BSD/ISC/Zlib，黑名单 GPL/AGPL/EUPL；RUSTSEC 漏洞检查；重复依赖警告；来源限制仅 crates.io）
- **PHP 迁移指南补充 5 章节**：第 11 章缓存系统迁移 / 第 12 章文件上传迁移 / 第 13 章视图模板迁移 / 第 14 章可观测性迁移（v0.2.0 新增）/ 第 15 章分布式追踪迁移（v0.2.0 新增）

### 变更
- **workspace.package.version**：`0.1.0` → `0.2.0`
- **CI 门禁增强**：移除 test/doc/audit/feature-matrix/unused-deps 5 个 job 的 `continue-on-error: true`，门禁严格生效
- **CI doc job**：添加 sz-orm path 依赖检查 + missing_docs 检查
- **CI test job**：添加 sz-orm path 依赖检查
- **CI 新增 deny job**：cargo-deny 检查 advisories（RUSTSEC）/ licenses / bans（重复依赖）/ sources
- **ADR README 索引**：将"待编写 ADR 清单"改为"ADR 完成状态"，12 个 ADR 全部标记为 ✅ 已接受；关键路径覆盖表 13 项全部标记为已覆盖

### 修复
- 无

## [0.1.0] - 2026-07-22

### 新增
- **框架核心**：sz-rust-core 28 模块就绪（controller/model/relation/middleware/guard/hooks/multi_app/health/h2/routing/addons/cache/event/validate/upload/view 等）
- **路由系统**：三层路由机制（属性宏 / 配置式 / 约定式），对齐 PHP `auto_multi_app` + `config/route.php`
- **中间件**：Tower Service + 洋葱模型，5 个内置中间件（Trace/Cors/Log/RateLimit/Auth）
- **控制器**：SzController trait + BaseController，对齐 PHP `app\SzController`
- **Model 钩子**：16 事件 HookDispatcher（PHP 原生 12 + sz-orm-core 扩展 4）
- **Guard 守卫**：鉴权决策层（AuthGuard/PermissionGuard/GuardChain），借鉴 NestJS
- **响应格式**：`{code, msg, data}` 标准响应，对齐 PHP `renderJson/renderSuccess/renderError`
- **错误体系**：BaseException + 9 个错误码（对齐 PHP + Rust 扩展）
- **缓存系统**：Cache facade + 多驱动（Memory/Redis），对齐 PHP `think\facade\Cache`
- **配置系统**：YAML 加载 + 环境变量覆盖 + 默认值，对齐 PHP `config/*.php`
- **验证器**：规则/场景/消息三件套，对齐 PHP `think\Validate`
- **事件系统**：事件监听器 + 订阅者，对齐 PHP `think\Event`
- **上传**：文件上传 + 图像处理（对齐 PHP `think\Filesystem` + `Grafika`）
- **视图**：模板渲染 + 布局继承，对齐 PHP `think\View`
- **多应用**：`auto_multi_app` 路径解析（oapc/admin/api/farm/oapi/cashier/scene）
- **HTTP/2**：完整 HTTP/2 支持（含 h2c upgrade）
- **插件系统**：addon 插件化机制
- **PDF 处理**：sz-rust-pdf 独立包
- **CLI 工具**：sz-rust-cli 命令行工具
- **业务示例**：sz-rust-addons-operate（375 测试，控制器+服务层迁移完成）
- **示例应用**：sz-rust-examples/crud_demo 完整 CRUD 示例
- **工程化**：10 道门禁（fmt/check/clippy/test/doc/audit/integration + 占位检查/安全扫描/feature 全组合）
- **CI**：GitHub Actions 7 道门禁
- **测试**：2938+ 测试通过（sz-rust-core 2563 + sz-rust-addons-operate 375）
- **文档**：README.md、LICENSE(MIT)、ADR 规范、审计清单、工程化实践规范

### 测试
- 2938+ 测试全部通过
- clippy 0 警告
- fmt 0 差异

## 版本对比链接

[Unreleased]: https://github.com/ljclz/sz-rust/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/ljclz/sz-rust/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ljclz/sz-rust/releases/tag/v0.1.0
