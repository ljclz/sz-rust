# SZ-Rust 变更日志

本文件记录 SZ-Rust 框架的所有显著变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

---

## [v1.2.0] — 2026-09-14 — 生产化增强 + 安全加固 + 生态扩展

### 概要

v1.1.0 至 v1.2.0 期间共 193 个 commit，涵盖：serve 命令生产化增强（7 大特性）、Admin 后台管理插件、多租户 SaaS 支持、Marketplace 插件市场、Frontend Codegen 可视化前端、AI 能力补齐、开源/企业版物理分离、覆盖率达标 85%+、幻影交付清零、安全审计修复（白帽+黑帽）、变异测试 3 轮补强、CI 门禁强化、可靠任务队列、多后端迁移支持。

### 新增

- **serve 命令生产化增强**（`packages/sz-rust-cli/src/cmd/serve.rs`）：多 worker runtime、优雅关闭超时、健康检查接线、访问日志中间件、TLS/HTTPS 支持、配置热重载、信号处理、Windows CLI 子命令（commit `ec65d31`）
- **Admin 后台管理插件**（`packages/sz-rust-addons-admin/`）：7 模块 21 端点 + 17 Capability + CLI 接线（`admin migrate/list-routes/list-capabilities/init`）（commit `c96196e`, `f6b0e7e`）
- **多租户 SaaS 支持**（`packages/sz-rust-core/src/multi_tenant.rs`）：tenant_id 自动注入 + 行级隔离 + 租户管理 API + 配置隔离（commit `2b149b6`）
- **数据权限扩展**（`packages/sz-rust-auth-facade/src/data_permission.rs`）：动态策略热更新 + 配置文件加载 + 管理后台 API（commit `69568fa`）
- **Marketplace 插件市场**（`packages/sz-rust-marketplace/`）：crate 骨架 + 数据访问层 + Web 平台（axum + JWT + OpenAPI）+ CLI 接线 + Docker Compose + e2e 测试（commit `bcca87e` ~ `a9d2cd6`）
- **Frontend Codegen 可视化前端**（`packages/sz-rust-frontend-codegen/`）：Vue 3 前端应用 + Pinia store + Tauri API 集成 + 12 个 e2e 测试（commit `5abefb5`, `ca19a2f`）
- **AI 能力补齐**：RagPipeline + Agent + LongTermMemory + LocalEmbedding + tiktoken + LlmChatCapability + Ai::embed/stream_chat + McpToolBridge + AiMetrics（commit `3c9047d`, `85a709f`, `8d2d1e9`）
- **可靠任务队列 JobQueue**（`packages/sz-rust-orm-facade/src/job_queue.rs`）：持久化 Job 表 + 状态机 + 原子领取（SKIP LOCKED）+ 退避重试 + 幂等 + 死信（commit `df67fc3`）
- **多后端数据库迁移**：admin migrate/init 全 5 后端（PostgreSQL/MySQL/SQLite/Oracle/MSSQL）（commit `4a63a5a`）
- **性能压测基线**（`packages/sz-rust-core/benches/`）：sz300 生产压测报告 + ab 压测基线（commit `ed44087`）
- **PR 审查 Skill**（`.trae/skills/sz-rust-pr-review/`）：状态机 + 5 环节 + 严重度模型 + AI 评审多 Provider 支持（commit `6a2dc58`, `9100a09`）
- **变异测试 3 轮补强**：hot_reload 6 个 missed 捕获，最终 missed=4（commit `b29aac8`）

### 变更

- **开源/企业版物理分离**：37→29 crate + Apache-2.0 许可证 + crates.io 发布 v1.2.0（commit `1614e84`, `b0fda63`）
- **sz-orm 依赖升级**：4.7.0 → 5.0.0 → 6.2.0（commit `69b6826`, `5f52aea`）
- **覆盖率达标 85%+**：11 crate 从基线提升至目标，workspace 总覆盖率 91.44%（commit `9a41345`, `f0375a0`）
- **幻影交付清零**：多轮审查清理虚构 crate、占位测试、未接线功能（commit `776fdf0`, `8767c09`, `a6928b9`）
- **铁律门禁上线**：51 处生产裸 unwrap → 0；生产 std::fs → tokio::fs 全链 async 化（commit `b9b22d9`, `78de034`）
- **CI 门禁强化**：覆盖率 CI 集成、cargo-deny 修复、cargo-machete 清理 49 个未使用依赖、Windows 兼容性、Miri、udeps（commit `776e423`, `7260ef9`, `d1ed1a2`）
- **Docker 镜像构建验证**：Dockerfile Rust 1.82→1.98 + ARM64 OpenSSL 交叉编译 + 监控栈端口绑定 127.0.0.1（commit `2bff51c`, `4542eef`）
- **连接池调优**：DB_POOL_* 环境变量 + 预热 + 动态扩缩容 + /metrics 池指标（commit `e23af8a`）

### 安全修复

- **白帽审计 H-1/H-2/M-1/M-2/M-4 修复**：中高危漏洞修复（commit `98ec48e`, `1573ef9`）
- **黑帽审计 A6 越权端点修复**：5 个越权遗漏端点（commit `2e45dc1`）
- **白帽审计 L-1/L-2/L-3 低危修复**（commit `747bf31`）
- **AiProviderConfig api_key 脱敏**（铁律 7）（commit `f4fda55`）
- **SQL 注入修复**：admin.rs format! 拼接 → 参数化查询（P0 安全修复）
- **明文密码输出修复**：admin.rs 移除密码打印（P0 安全修复）
- **连接串泄露修复**：marketplace/main.rs 日志不再打印含密码 URL（P0 安全修复）
- **硬编码凭据警告**：DATABASE_URL 和 JWT_SECRET 默认值添加警告日志（P1 安全修复）
- **reqwest 无超时修复**：3 处 Client::new() → Client::builder().timeout(30s)（P1 安全修复）
- **请求 DTO 手工 Debug 脱敏**（commit `d14f9eb`）
- **redis_tls 环境变量竞态修复**（commit `6863cf5`）
- **MemPool 区域分配器 API 收紧为 unsafe fn**（ADR-037）（commit `c6e6ff6`）

### 测试

- workspace 总测试数 2,633 → 4,800+（含 e2e/集成/单元）
- 变异测试 missed=4（3 轮补强）
- 4 个无断言测试补行为断言（铁律 10/23）
- plugin 命令行为集成测试 9 个（mockito 端到端）
- Admin 插件端到端验证测试 9 个
- services 层补测 17 测试（覆盖率 39.89%→44.75%）

### 文档

- 生产就绪与真实能力对比报告（基于 35 crate 实际验证）
- 项目成熟度评估报告（2026-09-04）
- ADR-037 MemPool unsafe API、ADR-038 k8s-operator 孤儿 crate 物理删除
- 覆盖率基线报告（91.44%）
- 审查报告归档（多轮，全绿）

---

## [v1.1.0] — 2026-08-10 — Admin Monitor API

### 概要

实现 `docs/cases/fssadmin-competitive-analysis.md` 中的 Admin Monitor API 需求，提供服务器信息、数据库连接池、Redis 状态三个管理端监控端点，路由级 RoleGuard 鉴权，`admin` feature 默认关闭不影响现有部署。

### 新增

- **`sz-rust-observability::admin` 模块**（`admin` feature）：
  - `sysinfo_collector`：`collect_server_info()` 基于 `sysinfo` crate 0.32，跨平台（Windows `COMPUTERNAME` / Unix `hostname`），`once_cell` 懒加载 rustc 版本
  - `db_pool_collector`：`DbPoolStats` trait（trait object 适配，observability crate 不直连 sz-orm-core）
  - `redis_collector`：`RedisStats` trait + INFO 解析，无连接时降级
- **`sz-rust-sz300` 集成**：`AppState` 新增 `db_pool_stats` + `redis_stats`；`DbPoolStatsAdapter` / `RedisStatsAdapter` 桥接具体实现
- **`RoleGuard` 中间件**：路由级角色校验，401（无/无效令牌）/ 403（缺角色）
- **3 个端点**：`GET /api/admin/server/info`、`GET /api/admin/db/pool`、`GET /api/admin/redis/info`

### 测试

- 20 个单元测试（observability 16 + role_guard 4），全部通过
- `cargo check -p sz-rust-sz300 --features admin` 与默认配置均编译通过

---

## [v0.6.7+] — 2026-08-09 — P0-P4 + 基础设施优化

### 概要

完成评估报告"七、后续发展方向建议"中 P0-P4 全部 18 项任务，以及基础设施工具链优化（文档同步规则 + Soak 自托管 + 同条件性能对比）。

### 新增

- **P0-1 服务器真实数据全链路验证**：sz-pay + sz-rust-sz300 连接生产 MySQL/PostgreSQL/Redis，E2E 全通过（27 条 file:line 证据）
- **P0-2 Redis 存储后端压测**：13 轮压测，高并发 QPS=30598~85500，19 条证据
- **P0-3 渗透测试**：29/29 测试通过，7 场景 × 4-5 用例，20 项防护机制（`packages/sz-rust-auth-facade/tests/security_pentest.rs`）
- **P1-4 RedisDegradationStore**：SET EX/GET/DEL/SCAN 实现（`packages/sz-rust-auth-facade/src/redis_store.rs`）
- **P1-5 RedisAuditStore**：ZADD/ZREVRANGE/ZRANGEBYSCORE Sorted Set 实现
- **P1-6 RedisTicketStore**：SET EX/GETDEL pipeline 原子 take + TTL 实现
- **P2-9 性能压测**：11 个新增 bench（`packages/sz-rust-auth-facade/benches/sso_bench.rs`）
- **P3-12 文档国际化**：9 个英文版文件（README.en.md × 8 + ADR 索引）
- **P3-13 addons 模板**：CMS（文章/分类/标签）+ Forum（板块/帖子/回复）+ IM（会话/消息/用户状态）3 个新包
- **P3-14 框架对比**：更新至 v0.6.7，新增 Poem 框架对比
- **P4-15 并发 10K 压测脚本**：`docs/spec/p4-stress-test/p4-15-concurrent-10k.js`
- **P4-16 100W Token 基准**：350K 签发/s, 435K 校验/s
- **文档同步强制规则**：project_rules.md 新增规则 19-22（`.trae/rules/project_rules.md:75-103`），AGENTS.md 新增文档同步约束小节（`AGENTS.md:62-70`），文档欠债清单（`docs/audit/doc-debt.md`）
- **Soak Test 自托管**：GitHub Actions soak.yml/soak-nightly.yml 迁移到服务器 cron 调度（`scripts/soak-self-hosted/`），10s 冒烟验证通过，cron 每周日 00:00 UTC + 每日 18:00 UTC 自动 6h soak
- **同条件性能对比环境**：5 个框架标准化压测目标（`scripts/perf-compare/benchmarks/`），服务器 Rust 升级到 1.97.1，wrk 4.1.0 + k6 v2.0.0 已安装，部分实测数据已获得（sz-rust 160K req/s, actix 192K req/s）

### 变更

- **P2-11 连接池调优**：PostgreSQL max_connections=10 + acquire_timeout=30s（`packages/sz-rust-sz300/src/db.rs`）
- **P2-10 SIMD 路由**：已评估，axum 0.8 matchit ~59ns，无需 SIMD 加速
- **评估报告全面更新**：劣势 3 项已解决，P0-P4 状态全部标记

### 跳过

- **P1-7 OAuth2 完整流程**：当前 generic OAuth2 满足 sz-pay 需求
- **P1-8 OpenTelemetry 集成**：W3C TraceContext 已满足当前需求

---

## [v0.3.1] — 2026-08-05 — 生产就绪修复

### 概要

从综合评分 55/100 提升到可生产状态。执行 8 阶段 / 24 主任务 / 62 子任务的修复计划，覆盖联编修复、依赖治理、失实声称撤回、限流熔断落地、性能基线建立、CI 门禁强化、文档更新。

### 新增

- **限流器**（`packages/sz-rust-middleware-facade/src/rate_limit.rs`）
  - `TokenBucket`：令牌桶算法，parking_lot::Mutex 保护，`with_max_keys` OOM 防护
  - `SlidingWindow`：滑动窗口算法，时间戳清理 + `with_max_keys` OOM 防护
  - 两者实现 `sz_rust_orm_facade::RateLimiter` trait（非 re-export sz-orm-limit）
  - 11 个单元测试（基础放行/拒绝、时间补充、独立 key、reset、OOM 防护、100 并发无误差）

- **熔断器**（`packages/sz-rust-middleware-facade/src/circuit_breaker.rs`）
  - `CircuitState` enum（Closed/Open/HalfOpen）
  - `CircuitBreakerConfig`（error_threshold / cooldown / probe_requests / stat_window）
  - `CircuitBreaker`（parking_lot::Mutex 保护状态机）
  - `circuit_breaker_middleware`（Open 返回 503，Closed/HalfOpen 放行）
  - 10 个单元测试（三态流转、错误率边界、并发安全、探测限制）

- **性能基线**（`packages/sz-rust-core/benches/`）
  - `collect_env.rs`：`BenchEnvironment` 结构体 + `collect()` + `write_to_path()` 元数据采集
  - `results/README.md`：结果目录说明 + 命名规范
  - `core_bench.rs` 新增 2 个基准组：`bench_rate_limiting` + `bench_circuit_breaker`
  - 9 类基准覆盖（超出 7 类最低要求）：route_matching / handler_ref_parse / route_config / json_serialization / middleware_chain / di_container / framework_vs_native / rate_limiting / circuit_breaker

- **CI 门禁强化**
  - `benchmark.yml`：新增 bench 基准覆盖检查（校验 9 类基准存在）
  - `security.yml`：新增 unmaintained 真实编译检查（paste / rustls-pemfile / rkyv 逐个 `cargo tree -i` 验证）
  - `ci.yml`：新增 Windows 兼容性 job（`CARGO_INCREMENTAL=0` 规避 STATUS_STACK_BUFFER_OVERRUN）

### 变更

- **依赖治理**
  - `paste` 消除：升级 imageproc 0.25→0.27（default-features=false）+ image 禁用 exr feature，切断 image→exr→pulp→paste 依赖链
  - `rustls-pemfile` 消除：迁移至 `rustls-pki-types::pem::PemObject`（`h2.rs` 中 PEM 解析代码）
  - `ttf-parser` 部分消除：lopdf 0.42→0.44 消除 lopdf 路径；ab_glyph→owned_ttf_parser 路径无替代品（已在 deny.toml 记录理由）
  - `rkyv` 确认为幻影依赖（`cargo tree -i rkyv` = nothing to print，含 --all-features）
  - `sz-rust-core` 清理：移除未使用的 imageproc / ab_glyph 依赖（实际使用在 infra-facade）
  - `deny.toml` + `audit.toml`：更新所有 ignore 条目（paste / rustls-pemfile / ttf-parser / rkyv）附理由

- **版本 bump**：workspace `Cargo.toml` version 0.3.0 → 0.3.1

- **文档更新**
  - `README.md`：版本号更新、限流熔断标注"已落地"、addons 标注脚手架状态、CI 门禁数 10→17
  - `docs/audit/2026-08-04-成熟度评估与生产差距.md`：修正 3 处失实声称（sz-protocol / sz-orm-dtx / 限流熔断）
  - `docs/audit/2026-08-04-代码实测深度评测.md`：限流熔断状态更新为"已落地"
  - `docs/audit/2026-08-05-项目状态评估报告.md`：限流熔断状态更新、版本号更新
  - `docs/sz-rust-engineering-practices.md`：版本号更新、测试数更新

### 撤回

- **失实声称撤回**（`docs/audit/2026-08-04-成熟度评估与生产差距.md`）
  - sz-protocol "护城河" → "已撤回：目录空，Cargo.toml 无引用"
  - sz-orm-dtx 分布式事务 → "Roadmap：无 src/ 实现"
  - 智能秤行业框架 → "Roadmap：sz-protocol 当前不存在"
  - 限流/熔断/降级 → "v0.3.1 已落地"

### 脚手架标注

- `packages/sz-rust-addons-crm/src/lib.rs`：添加脚手架标注 doc-comment
- `packages/sz-rust-addons-erp/src/lib.rs`：添加脚手架标注 doc-comment
- `packages/sz-rust-addons-ecommerce/src/lib.rs`：添加脚手架标注 doc-comment

### 测试

- workspace 联编：`cargo test --workspace --no-run` 退出码 0
- 逐包测试基线：core 424, infra 670, middleware 440(+21 新增), state 222, operate 466, loader 227, cli 184
- **总计 2,633 测试，0 失败**

### 约束遵守

- 未修改上游 `../sz-orm/` 仓库任何文件（sz-pay 依赖硬约束）
- 未删除任何有效测试
- Windows 环境全程使用 `CARGO_INCREMENTAL=0`
- 所有结论附 file:line 证据

---

## [v0.3.0] — 2026-08-02 — addons 生态 + RouterBuilder 泛型状态

（见历史提交记录）

---

## [v0.2.0] — 可观测性 + 分布式追踪

（见历史提交记录）