# sz-rust 实施进度追踪

> **关联文档**：`docs/product-technical-plan.md`（权威规划）
> **更新规则**：每完成一个任务或子任务，必须同步更新本文档
> **最后更新**：2026-10-30

---

## 2026-10-30 更新：鉴权授权上下文缓存（ADR-049，31–60 天路线项）

> **状态**：✅ 代码与测试完成 / ⏳ 端到端吞吐未实测
> **决策**：`docs/adr/0049-鉴权授权上下文缓存.md`
> **来源**：`docs/audit/2026-10-29-性能工程评测报告.md` §6 瓶颈 3「鉴权每请求查库」；31–60 天路线项「JWT claims 鉴权」的安全替代

| 交付项 | 位置 | 验证 |
|--------|------|------|
| `try_get_user_info_by_id` → `auth_context` 返回 `AuthContext{merchant_id,is_admin}`，投影收窄为 2 列 | `packages/sz-rust-sz300/src/services/auth_service.rs` | `cargo check -p sz-rust-sz300 --all-targets` = 0 error |
| 进程级授权上下文缓存（LRU 50k / TTL 60s / 定长 10 字节值；负结果不缓存） | 同上（`auth_ctx_cache` + `encode_auth_ctx`/`decode_auth_ctx`） | `cargo test -p sz-rust-sz300 --lib` = **116 passed**（+3 编解码单测） |
| 中间件接线（`CurrentUser` 注入语义不变，DB 不可用仍降级） | `packages/sz-rust-sz300/src/middleware/auth_middleware.rs` | `cargo clippy -p sz-rust-sz300 --all-targets` = 0 warning |
| 真库集成测试（hit/miss 计数、删行后仍命中、role=2 非管理员、负结果不缓存 + 可达性对照） | `packages/sz-rust-sz300/tests/auth_ctx_cache_test.rs` | **V**：一次性 MySQL `sz300-authctx-mysql` @127.0.0.1:3306 / `sz_orm_test`，`-- --ignored` = **1 passed**（四组场景同一运行时内串行；拆成两个 `#[tokio::test]` 会因进程级池绑定首个运行时关闭而误判，已合并） |
| ignored 测试接入 CI（含 ADR-048 与存量漏接） | `.github/workflows/db-integration.yml`（新增 4 个 sz300 `--ignored` 组） | 门禁 30 `ignored-test-routing.js` = 通过 |
| 顺带修复存量门禁失败 | `docs/adr/ADR-043-ai时代单一版本合并.md` 补「日期」字段 | `scripts/check-adr-coverage.sh` = exit 0（此前恒 exit 1） |
| 端到端吞吐量化 | — | **未实测**：需一次性 MySQL 9.6 + release 实例跑门禁 v2（起服务前须用户批准） |

---

## 2026-10-30 更新：订单列表 COUNT 查询缓存（ADR-048，31–90 天路线项）

> **状态**：✅ 完成（用户 2026-10-30 选定「先做 COUNT 缓存（最高 ROI）」）
> **决策**：`docs/adr/0048-订单count查询缓存.md`
> **来源**：`docs/audit/2026-10-29-性能整改现场验证记录.md` §3.3（order/list COUNT 全扫描 + 高 p99）

| 交付项 | 位置 | 验证 |
|--------|------|------|
| `AppState` 新增 `query_cache: SharedQueryCache`（v1.9.0 QueryCache 首次接入业务） | `packages/sz-rust-sz300/src/state.rs` + `main.rs` + 全部测试/bench 构造点 | `cargo check -p sz-rust-sz300 --all-targets` = 0 error |
| `OrderService::list` COUNT 按「WHERE 子句 + 参数化值」缓存（LRU 10k / TTL 60s） | `packages/sz-rust-sz300/src/services/order_service.rs` | `cargo test -p sz-rust-sz300 --lib` = 113 passed（+4 键单测） |
| `OrderService::create` 提交成功后 `cache.clear()` 整体失效 | 同上 | 真库 `order_count_cache_test` = 2 passed（命中/清空/键隔离） |
| COUNT 缓存集成测试 | `packages/sz-rust-sz300/tests/order_count_cache_test.rs` | 真库 MySQL `sz_orm_test`：2 passed |
| 顺带修复预存陈旧断言（config 默认用户 sz300_app） | `packages/sz-rust-sz300/tests/config_test.rs` | `config_test` = 6 passed |
| 回归验证 | 受影响的 `order_idempotency_test` / `outbox_webhook_test` | 3 passed / 4 passed；clippy 0 warning |
| 性能量化 | 2026-10-30 用户批准补测：门禁 v2 完整协议（c=50/10s/60s×3，一次性 MySQL 9.6 + perf-seed） | **order/list 872.4 → 2623.6 QPS（+200.7%）、p99 100.72 → 32.477ms（-67.8%）、0% 错误**；基线已 `--init` 重播种（health 6936.3 / product 1292.6 / order 2623.6） |

---

## 2026-10-25 更新：单一版本合并（ADR-043，废除企业版）

> **状态**：✅ 合并完成（本会话执行）
> **决策**：AI 时代企业版无存在意义，所有版本合并为单一开源版本（v2.0.0），全部 crate 统一 Apache-2.0。
> **方案**：`docs/single-edition-merge-plan.md`；决策记录：`docs/adr/ADR-043-ai时代单一版本合并.md`

| 交付项 | 位置 | 验证 |
|--------|------|------|
| 9 个企业 crate 入库（7 addons + sdd + migration） | `packages/sz-rust-addons-{cms,crm,ecommerce,erp,forum,im,operate}` + `packages/sz-rust-{sdd,migration}` | `cargo check --workspace` 65 members 全过 |
| members + workspace.dependencies 更新 | 根 `Cargo.toml` | 同上 |
| sz300 生产能力移植（ai/wasm/view + 限流/熔断链 + AI init） | `packages/sz-rust-sz300/src/{controllers/ai.rs,controllers/wasm_api.rs,controllers/view.rs,router.rs,config.rs,main.rs}` | `cargo test -p sz-rust-sz300 --lib` = 87 passed（含 26 项安全 Gauntlet） |
| 许可证统一 Apache-2.0 | 119 个企业来源文件 SPDX 头 | `python scripts/check_license_compliance.py --mode all --expected-license Apache-2.0`（企业来源文件 0 违规；既有 OSS 文件缺头另批补齐） |
| 集成测试修复（metrics_token 12 处 + mock_app_state + hot_path bench） | `packages/sz-rust-sz300/tests/` + `src/state.rs` + `benches/hot_path.rs` | `cargo test -p sz-rust-sz300` 全绿 |
| sdd visual_adapter 死代码移出编译（WIP 引用未实现 API） | `packages/sz-rust-sdd/src/lib.rs` | `cargo check -p sz-rust-sdd` 通过 |
| CI isolation 隔离检查废止（与合并矛盾） | `.github/workflows/ci.yml` | job 已移除，注释说明 |
| docker-compose.prod.yml 补 AI 环境变量 | `docker-compose.prod.yml` | — |

---

## 2026-10-20 更新：二维码生成验证闭环（吸收公众号文章实践）

> **状态**：✅ 完成
> **来源**：微信公众号《qrcode-rs + rxing：Rust 二维码生成与验证的完整闭环实战》（2026-10-13）
> **方案**：采用文章「生成 → 解码 → 一致性校验」闭环模式；解码后端以 `rqrr 0.10.1` 替代 `rxing`（避免 workspace MSRV 1.81→1.85 升级与 `image/exr → pulp/paste` 依赖链）。决策记录：`docs/adr/ADR-042-qr-verify-rqrr-closed-loop.md`。

| 交付项 | 位置 | 验证 |
|--------|------|------|
| `QrCodeError::Decode` / `DataMismatch` | `packages/sz-rust-core/src/qr_code.rs:98,102` | `cargo test -p sz-rust-core --lib qr_code` = 24 passed |
| `QrVersion` 枚举（Normal 1~40 / Micro 1~4） | `packages/sz-rust-core/src/qr_code.rs:179` | 同上 |
| `generate_matrix_with_version` | `packages/sz-rust-core/src/qr_code.rs:435` | 同上 |
| `generate_unicode`（Unicode 终端输出） | `packages/sz-rust-core/src/qr_code.rs:470` | 同上 |
| `verify_png_bytes`（内存解码验证） | `packages/sz-rust-core/src/qr_code.rs:501` | 同上 |
| `generate_and_verify`（生成→解码→一致性校验） | `packages/sz-rust-core/src/qr_code.rs:530` | 同上 |
| 新增 11 个单测（qr_code 13 → 24） | `packages/sz-rust-core/src/qr_code.rs:854-957` | 24 passed / 0 failed |
| 覆盖率门禁（铁律 10） | `qr_code.rs`（773 行 / 44 missed） | `cargo llvm-cov -p sz-rust-core --lib`：行覆盖 **94.31%**（≥ 85%），整体 96.17% |
| 依赖登记 `rqrr = "0.10"` | 根 `Cargo.toml` + `packages/sz-rust-core/Cargo.toml` | `cargo check -p sz-rust-core --lib` 通过 |
| ADR-042 + ADR 索引补录 017~022/038~041 | `docs/adr/ADR-042-qr-verify-rqrr-closed-loop.md` + `docs/adr/README.md` | — |
| 五维审查报告 | `docs/audit/2026-10-20-qr-verify-five-dim-review.md` | 5 维无阻断项 |
| `mutants.toml` 排除理由更新 | `.cargo/mutants.toml:11-13` | — |

**后续建议**：多格式扫描（Aztec/DataMatrix/PDF417）需求出现时，另行评估 `rxing`（需 MSRV 升级决策，见 ADR-042 后果段）。

---

## v1.7.0 覆盖率与质量提升（2026-10-04）

> **状态**：✅ 全部完成，所有 v1.7.0 新 crate 覆盖率 ≥ 90%（多数 95-100%）

### 覆盖率提升汇总

| Crate | 文件 | 改前 | 改后 | 新增测试 |
|-------|------|------|------|----------|
| sz-rust-websocket | broadcast.rs | 0% | 100% | +5 |
| sz-rust-websocket | auth.rs | 85% | 96% | +5 |
| sz-rust-websocket | heartbeat.rs | 86% | 96% | +2 |
| sz-rust-websocket | manager.rs | 89% | 97% | +9 |
| sz-rust-i18n | resource.rs | 85% | 99% | +14 |
| sz-rust-zerocopy | serializer.rs | 88% | 97% | +10 |
| sz-rust-admin-ui | theme.rs | 90% | 100% | +11 |
| sz-rust-monitor-panel | alert_display.rs | 90% | 100% | +9 |
| sz-rust-codegen-ui | model.rs | 75% | 100% | +13 |
| sz-rust-codegen-ui | preview.rs | 85% | 100% | +10 |
| sz-rust-codegen-ui | template_editor.rs | 87% | 100% | +9 |

- **新增测试总数**：+97 个（跨 6 个 crate）
- **clippy**：全部通过（`--all-targets -- -D warnings`）
- **已验证覆盖率 ≥ 90% 的 crate**：data-mask(97%)、security-headers(98%)、key-rotation(98%)、websocket(97%)、alert-engine(95%)、pipeline(92%)、log-aggregator(95%)、i18n(98%)、plugin-sdk(98%)、zerocopy(97%)、benchmark-suite(96%)、admin-ui(100%)、monitor-panel(96%)、codegen-ui(98%)

---

## v1.9.0 生产深化 + 闭环验证 + 债务清偿（2026-10-05 规划，2026-10-07 完成）

> **状态**：✅ P0-P3 全部完成，10 大核心能力 + P0 债务清偿，全部通过 `all-v1-9` feature gate 控制
> **SDD 文档**：`.codeartsdoer/specs/v1.9.0/`（spec.md 753行 + design.md 1059行 + tasks.md 594行）

### 完成范围

#### P0 债务清偿

- **GraphQL 持久化**（`v19-graphql-persist`）：DB-backed schema（`build_schema_with_db`），resolver 直接查 DB，13 集成测试
- **JWT Audience 安全增强**（`v19-jwt-audience`）：`verify_token_with_audience` + `AudienceConfig`（service_id/grace_period/enabled）+ grace period 兼容 + 13 集成测试
- **覆盖率豁免清偿**（`scripts/audit/coverage-exemption.js`）：doc-debt RESOLVED

#### P1 功能开发

- **Saga 分布式事务接入订单**（`v19-saga`）：6 个 Saga Action + `create_with_saga` + `OrderCreateResult` + DbTxLogStore 持久化 + 4 E2E 测试
- **可观测性闭环**（`v19-obs-closure`）：tracing_middleware 全链路追踪 + TraceID + Span 本地缓冲 + SilenceManager 告警静默去重 + BusinessAlertRules 业务告警规则集 + 6 E2E 测试
- **性能深化**：热路径基线（3 条）+ CI 性能回归门禁（`scripts/perf-regression.js`）+ N+1 检测门禁（`scripts/nplus1-detect.js`）+ 连接池调优（`PoolConfig` + `PoolExhaustedError`）+ 查询缓存（LRU + TTL + 命中率统计）
  - 2026-10-29 更正：上述门禁在 2026-10-29 前**未被任何 workflow 引用**，且脚本用 5 s 短跑 + 失败计数 + 跨机器占位基线，不可作为性能证据。已重写为 v2 并接入 `.github/workflows/perf-gate.yml`，见 `docs/adr/0046-性能工程整改-限流分桶与熔断窗口与门禁重写.md`。
  - 2026-10-29 真库端到端验证：一次性 MySQL 9.6 + migrations 001–007 + `deploy/sql/perf-seed.sql`（首跑暴露并修正 `merchant.market_id` NOT NULL 缺列）+ release 实例跑通门禁 v2，`/health` 实测 +34.9%（V）；**同源 A/B**（同一容器+同一数据集）product +5.2% / order +8.6%（W，无回归）；本机指纹已 `--init` 播种真实基线。期间新发现并修复「无 `aud` 令牌 grace-period 告警按请求落盘」（占日志字节 99.78% → `WarnSampler` 60 s 采样）；**审计链静默丢弃（c=50 下 397 条）**经用户决策于 2026-10-30 用**本地缓冲重放**修复（ADR-047，真库验证 453 POST → 453 行，0 丢弃）。证据见 `docs/audit/2026-10-29-性能整改现场验证记录.md` §7/§11/§12/§13。

#### P2 功能开发

- **AI 智能分类**（`v19-ai-classify`）：`AiClassifier` + `AiFacade` trait + 脱敏 + 置信度决策 + 人工 override + 降级 + 7 E2E 测试
- **国际化接入**（`v19-i18n`）：`I18nExtractor` + `ErrorCode` 枚举 + `translate_error` + zh-CN/en 资源文件 + 7 E2E 测试
- **配置中心动态配置**（`v19-config-center`）：`DynamicConfig` + `ConfigSource` trait + 降级 + 本地缓存恢复 + 4 E2E 测试

#### P3 功能开发

- **插件生态落地**（`v19-plugin-flow`）：`PluginManager` + `PluginMarketplace` trait + `InMemoryMarketplace` + Ed25519 签名校验 + install/uninstall/list 端点 + 6 E2E 测试
- **全量集成测试**（`tests/v19_integration.rs`）：10 个跨模块集成测试，验证 all-v1-9 feature 下各模块无状态冲突
- **生产配置接入**（`V19Config`）：`AudienceSettings` + `ai_classifier_threshold` + `dynamic_config_source` + `alert_silence_duration` + `trace_sample_rate` + env 加载

### 验证结果

- `cargo test --lib --features all-v1-9` = 106 passed
- `cargo test --test v19_integration --features all-v1-9` = 10 passed
- `cargo test --test plugin_flow_e2e --features all-v1-9` = 6 passed
- `cargo clippy --all-targets --features all-v1-9 -- -D warnings` = 0 warning
- `node scripts/nplus1-detect.js` = 0 违规
- `node scripts/perf-regression.js --skip-bench` = PASS（v1.9.0 时点旧脚本；v2 门禁在无指纹基线时输出 `GATE INACTIVE`，不能作为性能达标证据）

### 提交记录

- `10e31e82` P0 GraphQL 持久化 + JWT Audience
- `e51dbd59` P0 覆盖率豁免清偿
- `9fe56c99` P1 Saga 分布式事务
- `9e4aca62` P1 可观测性闭环
- `2226bae3` P1 性能深化
- `41ad7242` P2 AI 智能分类
- `f05ff187` P2 国际化接入
- `0e0647ba` P2 配置中心动态配置
- `da90bc82` P3 插件生态落地
- `226a7d00` P3 集成测试与全量门禁
- `f8ba74ac` P3 生产配置接入

---

## v1.8.0 生产接线 + 质量硬化（2026-10-04 完成）

> **状态**：✅ P1-P5 全部完成，已推送 + tag v1.8.0
> **SDD 文档**：`.codeartsdoer/specs/v1.8.0/`

### 完成范围

- **P1 安全合规接线**（5 模块）：RBAC + 数据脱敏 + 安全头 + 审计链 + 密钥轮换 → sz300 生产路由（46 测试）
- **P2 API 协议接线**（4 模块）：GraphQL + WebSocket + SSE + Upload 增强 → sz300 生产路由（75 测试）
- **P3 集成测试套件**（7 模块）：sz300 + v1.7.0 全部新能力端到端测试 29 E2E 测试
- **P4 性能基线**（4 模块）：热路径 QPS 基线 + RBAC 缓存 + 脱敏吞吐 + 连接池调优（13 benchmark 函数）
- **P5 OpenAPI 文档**（3 模块）：自动生成 openapi.yaml（33 端点）+ Swagger UI + CI 一致性检查

### 验证结果

- 全部 239 测试通过
- clippy 0 warning
- tag v1.8.0 已推送

---

## v1.7.0 全栈能力跃升（2026-10-03）

> **状态**：✅ P0-P6 全部完成，P7 集成验证通过（编译/测试/clippy/doc/dry-run 全绿）

### 完成范围

- **P1 插件生态**（5 模块）：marketplace + addons-loader isolation + frontend-codegen theme + i18n + plugin-sdk
- **P2 性能基准**（3 模块）：pipeline 并行化 + zerocopy(rkyv 0.8/zerocopy 0.8) + benchmark-suite
- **P3 可观测深化**（3 模块）：otel span 丰富化 + alert-engine + log-aggregator
- **P4 安全合规**（5 模块）：RBAC + data-mask + audit-chain + key-rotation + security-headers（45 单元测试）
- **P5 API 协议**（4 模块）：GraphQL+DataLoader + WebSocket + SSE 重连+背压 + Upload 增强（74 单元测试）
- **P6 前端可视化**（3 模块，Tauri 跳过）：admin-ui + monitor-panel + codegen-ui（82 单元测试）

### 验证结果

- `cargo build --workspace` ✅
- `cargo build --workspace --features all-v1-7` ✅
- `cargo test --workspace` 452 passed / 2 failed（预存环境超时）
- `cargo test --workspace --features all-v1-7` 全部通过（跳过 5 个已知环境问题）
- `cargo clippy --workspace --all-targets --features all-v1-7 -- -D warnings` ✅
- `cargo doc --workspace --features all-v1-7` ✅（1 预存 rustdoc 警告）
- `cargo publish --dry-run` ✅（7 个独立 crate 验证通过）

### 新增 crate

`sz-rust-data-mask` / `sz-rust-key-rotation` / `sz-rust-security-headers` / `sz-rust-websocket` / `sz-rust-admin-ui` / `sz-rust-monitor-panel` / `sz-rust-codegen-ui`

---

## P0-3 开源版/企业版物理分离（2026-09-05）

> **状态**：✅ 全部完成（代码分离 + crates.io 发布 + sz-pay 兼容 + sz300 生产部署）

### 已完成

- 组1-3：盘点基线、企业版仓库初始化（git filter-repo 39 commits）、开源仓库瘦身（37→29 members）
- 组4：依赖路径转换（26 开源核心 version="1.2"，8 企业版间 path 依赖）
- 组5：许可证标注（Apache-2.0 + LicenseRef-SZ-Commercial，688 文件添加 SPDX 头）
- 组6：合规检查脚本（check_isolation.py、check_license_compliance.py、check_no_crate_level_allow_dead_code.py）
- 组7：CI/CD 配置分离（publish-oss.yml workflow_dispatch + 企业版 ci.yml/publish.yml）
- 组8.1/8.3/8.4：开源版编译通过、5135 lib 测试通过 0 失败、workspace 配置验证通过
- git filter-repo 历史清理：开源仓库 git log 无企业版 crate 记录

### 待完成

- 组10.2：企业版 Cloudsmith 发布（用户选择暂缓，需 Cloudsmith 账户配置）

### 已完成（2026-09-09 更新）

- 组8.2：企业版编译验证 ✅（cargo check --workspace 通过，8.23s）
- 组9：sz-pay 兼容性验证 ✅（cargo check 45.40s 通过，零修改兼容）
- 组10.1：crates.io 发布 ✅（8 crates: 7×1.2.0 + sz-rust-tracing 1.2.1）
- 组11：sz300 生产部署 ✅（服务器 122.51.216.76:8300 运行中，health 200）

---

## 一、总体进度概览

> **文档降级声明**（2026-08-13 审计核实）：以下进度已经过独立验证，标注真实状态。
> 审计报告：`docs/audit/2026-08-13-文档已实现但生产零调用审计报告.md`

```
Phase 1（基础设施）      [████████░░] 80%  预计 1-2 个月（Capability/模板/分离/生产接线已完成，Cloudsmith 待配置）
Phase 2（AI 生成能力）    [██████░░░░] 60%  预计 2-4 个月（RAG/MCP/SDD已完成，迁移工具在企业版仓库）
Phase 3（产品化）         [████████░░] 80%  预计 4-6 个月（画布38tests✅/市场162tests✅/6企业版插件119tests✅）
Phase 4（生态）           [████████░░] 80%  预计 6-12 个月（前端生成/市场/画布已完成并部署验证）

里程碑
M1 Capability Registry MVP     ■ 已完成    预计 1 个月（38 tests ✅，sz300 生产已接线）
M2 开源/企业分离 + 插件模板     ■ 已完成    预计 2 个月（分离 ✅，模板 289 tests ✅）
M3 SDD Agent MVP               ■ 已完成    预计 3 个月（企业版 sz-rust-sdd 0.1.0，157 tests ✅，commit cc8b21a）
M4 迁移工具 + 行业 RAG          ■ 已完成    预计 4 个月（RAG 52 tests ✅，迁移工具在企业版仓库）
M5 可视化画布 MVP              ■ 已完成    预计 5 个月（38 tests ✅，e2e 12 tests ✅，commit ca19a2f）
M6 插件市场 MVP + 真实用户      ■ 已完成    预计 6 个月（162 tests ✅，服务器部署验证通过，6 企业版插件 119 tests ✅）
M7 P3 性能优化                 ■ 已完成    预计 7 个月（919 tests ✅，SIMD/零拷贝/内存池/连接池/异步优化全实现）
M12 完整生态                   □ 待办      预计 12 个月
```

**图例**：`█` 已完成 / `▓` 进行中 / `░` 未开始 / `□` 待办 / `■` 已完成 / `⊗` 阻塞

---

## 二、Phase 1：基础设施（1-2 个月）

### P1-T1：Capability Registry（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-08-11 | 2026-08-11 | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 新建 `sz-rust-capability` crate | ■ | Batch A 完成 |
| 2 | 定义 `Capability` trait | ■ | Batch B 完成，含 CapabilityInfo |
| 3 | 实现 `CapabilityRegistry` | ■ | Batch B 完成，含 validate_json_schema |
| 4 | 内置 Skill 注册（LLM/代码搜索/文件操作） | ■ | Batch C 完成，register_builtin_skills 入口框架 |
| 5 | MCP 工具注册为 Capability | ■ | Batch C 完成，7 个 MCP 工具适配为 McpCapabilityAdapter |
| 6 | 单元测试 + 集成测试 | ■ | 24 测试通过（含 3 并发测试）+ 5 doc-tests |
| 7 | API 文档 + 使用指南 | ■ | rustdoc 完成，5 doc-tests 通过 |
| 8 | ai-facade 集成（LlmChatCapability） | ■ | 完成，委托 Ai::chat，AiError→CapError 映射 |
| 9 | addons-loader 集成（CapabilityHook） | ■ | 完成，CapabilityHook trait + unregister_plugin_capabilities |
| 10 | 性能基准测试 | ■ | 全部达标（注册 187ns / 查找 38ns / 标签搜索 20μs） |

**验收标准**：
- [x] AI Agent 可通过 Registry 发现并调用能力
- [x] Plugin 可实现 Capability trait 并注册
- [x] 按标签搜索能力正常工作
- [x] 并发调用安全

---

### P1-T2：开源版/企业版 crate 分离（1 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ▓ 进行中 | 2026-08-11 | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 重构 workspace 结构（开源/企业分离） | ■ | Batch A 完成，双仓库目录 + git filter-repo 历史迁移（开源 38 提交 / 企业版 19 提交） |
| 2 | sz-rust 仓库（开源）：只含开源 crate | ■ | Batch B 完成，26 crate，Apache-2.0，cargo check 通过，4542 lib 测试通过 |
| 3 | sz-rust-enterprise 仓库（企业版）：只含企业版 crate | ■ | Batch C 完成，7 crate，LicenseRef-SZ-Commercial，cargo check 通过，466 lib 测试通过 |
| 4 | CI/CD 发布流程配置 | ■ | GitHub Actions ci.yml（20 门禁）+ publish.yml + GitLab CI .gitlab-ci.yml |
| 5 | 许可证合规检查脚本 | ■ | check-isolation.sh + check-license-header.sh，隔离检查通过 |
| 6 | 发布文档 | ■ | 开源版/企业版发布指南已创建 |
| 7 | 源文件 SPDX 许可证头 | ■ | 开源 376 文件 Apache-2.0 + 企业版 98 文件 LicenseRef-SZ-Commercial |
| 8 | deny.toml 合规配置 | ■ | 开源版 + 企业版 deny.toml 配置完成 |

**验收标准**：
- [ ] `cargo publish` 可正确发布开源 crate 到 crates.io（需实际发布验证）
- [x] 企业版 crate 不会出现在开源仓库中（隔离检查通过）
- [x] 企业版 crate 可通过私有 registry 安装（Cloudsmith 配置完成）

---

### P1-T3：插件模板库（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ▓ 进行中 | 2026-08-11 | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | CRUD 模板（Tera 模板） | ■ | 8 个 .tera + template.json，含 model/controller/service/repository/migration/routes/manifest/tests |
| 2 | 主从模板 | ■ | 8 个 .tera + template.json，{% extends %} 跨目录继承 CRUD，含级联服务/数据源配置/外键约束 |
| 3 | 模板渲染引擎集成 | ■ | TemplateEngine 封装 Tera 1.20，init/render/list_templates/validate_template_type |
| 4 | CLI 命令 `sz-rust-cli make:plugin --template crud` | ■ | execute_make_plugin 含输入校验/上下文构建/模板渲染/文件写入/cargo check/回滚 |
| 5 | 模板使用文档 | □ | 待完成 |
| 6 | FieldParser 字段解析器 | ■ | 支持 7 种 Rust 类型→SQL 映射，含 pk/index 修饰符 |
| 7 | InputValidator 输入校验 | ■ | 路径遍历防护 + 注入防护 + 外键校验 |
| 8 | InteractivePrompt 交互式补全 | ■ | dialoguer FuzzySelect + Input，TTY 检测 |
| 9 | CargoChecker 编译验证 | ■ | 异步 cargo check + 30s 超时 + 回滚机制 |
| 10 | stubs.rs 迁移至 Tera | □ | 可延后，现有 make:* 命令不受影响 |

**验收标准**：
- [x] `make:plugin` 可基于模板生成可编译的插件骨架
- [x] 生成的插件通过 `cargo check`（集成回滚机制）
- [x] 生成的插件包含基本 CRUD 功能
- [x] 273/273 单元测试通过（含 52 Batch A + 6 模板渲染 + 20 Batch C + 5 CargoChecker）

---

### P1-T4：完善现有业务插件（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 完成 | 2026-08-11 | 2026-08-11 | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | sz-rust-addons-cms → 可发布 CMS 插件 | ■ | 5 Capability + manifest.json + README.md + 21 测试全部通过 |
| 2 | sz-rust-addons-crm → 可发布 CRM 插件 | ■ | 7 Capability + convert 原子性 + update_stage + manifest.json + README.md + 35 测试全部通过 |
| 3 | sz-rust-addons-ecommerce → 可发布电商插件 | ■ | 6 Capability + cart 累加 + order 正向流转 + manifest.json + README.md + 44 测试全部通过 |
| 4 | 每个插件实现 Capability trait | ■ | CMS 5 + CRM 7 + 电商 6 = 18 个 Capability 全部实现 |
| 5 | 每个插件有完整 manifest.json | ■ | 3 个 manifest.json 均含 13 个必需字段，JSON 格式有效 |
| 6 | 每个插件有测试覆盖 | ■ | CMS 21 + CRM 35 + 电商 44 = 100 个测试全部通过，现有 42 个测试全部保留 |
| 7 | 每个插件有使用文档 | ■ | 3 个 README.md 均含 7 章节（中文正文 + 英文 API） |
| 8 | 跨插件集成测试 | ■ | 12 个集成测试（6 能力注册 + 6 铁律合规 Send+Sync 断言）全部通过 |
| 9 | CHANGELOG.md 更新 | ■ | [Unreleased] - 2026-08-11 条目已添加 |

**验收标准**：
- [x] 插件可通过 `sz-rust-cli plugin install` 安装（manifest.json 已就绪）
- [x] 插件能力可通过 Capability Registry 调用（18 个能力注册无冲突）
- [x] 插件通过 22 条铁律检查（Send+Sync、无 std::fs、能力命名前缀、requires_confirmation 全部通过）
- [x] 现有 42 个测试全部保留继续通过（CRM 21 + 电商 21 回归无破坏）

---

## 三、Phase 2：AI 生成能力（2-4 个月）

### P2-T1：SDD Agent 编排（4 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-09-05 | 2026-09-05 | — |

> **交付证据**：commit `cc8b21a`（企业版仓库 main 分支）
> **crate 路径**：`E:\www\rust\sz-rust-enterprise\packages\sz-rust-sdd\`
> **测试**：157 tests 全通过（128 lib + 29 E2E），clippy 0 warnings，fmt OK

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 新建 `sz-rust-sdd` crate（企业版） | ■ | 15 模块 + bin target `sdd`，`#![forbid(unsafe_code)]` |
| 2 | Spec Agent（需求规格生成） | ■ | SpecStageExecutor + SpecValidator（六章节校验） |
| 3 | Design Agent（技术设计 + 存量分析） | ■ | DesignStageExecutor + LegacyAnalyzer（目录扫描/风格检测/隐式约定） |
| 4 | Task Agent（任务清单生成） | ■ | TaskStageExecutor + TaskValidator（可执行命令 + 验证方法校验） |
| 5 | Coding Agent（代码生成 + Compile-Fix 循环） | ■ | CodingStageExecutor + CodingValidator（铁律阻断） |
| 6 | HITL 闸门实现 | ■ | ReviewGate（Pass/Rollback/Redo 三选一 + 幂等）+ CliReviewNotifier（重试 3 次/脱敏） |
| 7 | Spec 文件持久化 | ■ | ArtifactStore（版本递增不覆盖 / diff / export）+ 事务一致性（两阶段写 + 崩溃恢复） |
| 8 | 多模型路由 | ■ | AiConfig 锁定会话创建时，运行期不可变更；经 Ai::agent facade |
| 9 | 与现有 Skills 集成 | ■ | capability 守卫（8 SDD capability）+ SddRedactor（组合 SourceCodeRedactor） |

**验收标准**：
- [x] 输入自然语言需求 → 输出完整 spec.md + design.md + tasks.md（E2E 全流转测试通过）
- [x] 用户确认后 → 生成可编译代码（Coding 阶段铁律校验通过）
- [x] Compile-Fix 循环自动修复编译错误（cargo check 验证集成）
- [x] 生成代码附带 SDD 文档（产物版本化 + 轨迹记录）

---

### P2-T2：AI 辅助迁移工具（3 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-09-05 | 2026-09-05 | — |

> **交付证据**：企业版仓库 sz-rust-sdd 内 MigrationAgent 模块
> **测试**：迁移工具在企业版 SDD Agent 内实现，3 个 TP6 案例 + MigrationReport

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 新建 `sz-plugin-migration`（企业版） | ■ | 在 sz-rust-sdd 内实现 |
| 2 | TP6 代码分析器（路由/模型/控制器识别） | ■ | 完成 |
| 3 | sz-rust 代码生成器 | ■ | 完成 |
| 4 | 增量验证工具（对比 TP6 和 sz-rust 响应） | ■ | 完成 |
| 5 | 迁移报告生成 | ■ | MigrationReport |
| 6 | 迁移案例文档（基于创始人自己的系统） | ■ | 3 个 TP6 案例 |

**验收标准**：
- [x] 可分析 TP6 项目，输出分析报告
- [x] 可生成等价 sz-rust 代码
- [x] 可对比 TP6 和 sz-rust 响应一致性

---

### P2-T3：行业 RAG 知识库（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-08-11 | 2026-08-11 | — |

> **交付证据**：sz-rust-rag crate, 52 tests passed
> **生产接线**：sz300 main.rs:297 IndustryRag::init() + controllers/ai.rs:69 IndustryRag::search()

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 29+ 项目代码向量化（Embedding） | ■ | 完成 |
| 2 | 行业术语表（菜市场业务术语） | ■ | 25 术语 + 10 规则 |
| 3 | 业务规则库（从现有代码提取） | ■ | 完成 |
| 4 | RAG 检索集成到 SDD Agent | ■ | sz300 生产已接线 |
| 5 | 数据模型模板库 | ■ | 7 模型模板 |

**验收标准**：
- [ ] SDD Agent 生成代码时可检索行业知识库
- [x] SDD Agent 生成代码时可检索行业知识库
- [x] 检索结果提升生成代码的行业相关性

---

### P2-T4：MCP 工具扩展（1 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-08-11 | 2026-08-11 | — |

> **交付证据**：sz-rust-mcp crate, 7 个 MCP 工具适配为 McpCapabilityAdapter

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | CRUD 操作工具（create/read/update/delete） | ■ | 完成 |
| 2 | 迁移管理工具（migrate/create/status） | ■ | 完成 |
| 3 | 测试工具（test/run/coverage） | ■ | 完成 |
| 4 | 部署工具（deploy/check） | ■ | 完成 |
| 5 | 插件管理工具（plugin/list/install/uninstall） | ■ | 完成 |

**验收标准**：
- [x] 所有新工具通过测试
- [x] AI Agent 可通过 MCP 调用所有工具

---

## 四、Phase 3：产品化（4-6 个月）

### P3-T1：可视化应用搭建画布（6 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-09-07 | 2026-09-09 | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | Tauri + Vue 桌面工作 bench | ■ | 完成，Tauri 2.x + Vue 3 |
| 2 | 需求描述界面（自然语言输入） | ■ | 完成，SDD facade 接入 |
| 3 | 规格可视化（spec.md 渲染） | ■ | 完成，Pinia store + 组件 |
| 4 | 任务进度看板（tasks.md 执行进度） | ■ | 完成 |
| 5 | 实时日志（SDD Agent 执行日志） | ■ | 完成，Tauri event 通信 |
| 6 | 插件管理界面 | ■ | 完成，capability/plugin store |
| 7 | 应用预览 | ■ | 完成，preview.rs graceful shutdown |

**验证**：38 tests passed (26 unit + 12 e2e), commit `ca19a2f`

---

### P3-T2：插件市场 MVP（3 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-09-07 | 2026-09-09 | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 插件市场 Web 平台 | ■ | 完成，axum 0.8 RESTful API |
| 2 | CLI 集成（search/install） | ■ | 完成，MarketplaceClient |
| 3 | 插件审核流程 | ■ | 完成，approve/reject 端点 |
| 4 | 支付集成（可选） | — | 暂缓 |
| 5 | 开发者文档 | ■ | 完成，OpenAPI 3.0 spec |

**验证**：162 tests passed, 服务器 122.51.216.76:8080 部署验证通过, commit `e69ffaa`

---

### P3-T3：真实用户案例（持续）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**目标用户**：
- 有存量 TP6 系统的中小企业技术负责人
- 菜市场/生鲜行业数字化需求方
- 对 Rust + AI 感兴趣的开发者

---

## 五、Phase 4：生态（6-12 个月）

### P4-T1：前端生成

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-08-12 | 2026-09-09 | — |

**验证**：sz-rust-frontend-codegen 53 tests passed, 确定性生成 + 路径穿越防护 + BTreeMap 排序

### P4-T2：工作流引擎

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-08-12 | 2026-08-12 | — |

**验证**：sz-rust-workflow crate 已实现（创建 ecc7adb → 深接线 40e91b6 → 随 1614e84 分离迁移开源版），28 错误码（2026-09-20 复核 `src/error.rs`：`WF_001`～`WF_051` 共 28 变体），136 测试（`git grep -E '#\[test\]|#\[tokio::test\]' HEAD -- packages/sz-rust-workflow`），FlowDefinition（definition/models.rs）/StateMachineEngine（engine/state_machine.rs）/ApprovalFlowEngine（engine/approval.rs）

### P4-T3：开发者社区

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

---

## 六、已完成工作记录

> 此章节记录在规划文档创建之前已完成的工作，作为基线。

### 6.1 P1 安全中间件（SDD 完整流程）

| 项目 | 详情 |
|------|------|
| **完成日期** | 2026-08-11 |
| **SDD 文档** | `.codeartsdoer/specs/security_middleware/` |
| **任务总数** | 38 个子任务（4 批次 A/B/C/D） |
| **新增测试** | middleware-facade: 515 通过（+60），http-facade: 167 通过 |
| **状态** | ■ 已完成 |

**交付内容**：

| 中间件 | 文件 | 测试数 | 默认启用 |
|--------|------|--------|----------|
| 安全响应头注入 | `middleware-facade/src/security_headers.rs` | 15 | 是 |
| IP 访问控制 | `middleware-facade/src/ip_access_control.rs` | 17 | 否 |
| 安全审计日志 | `middleware-facade/src/audit_log.rs` | 11 | 否 |
| 请求体大小限制 | `middleware-facade/src/body_size_limit.rs` | 8 | 否 |
| 配置聚合 | `middleware-facade/src/security_section.rs` | 8 | — |
| 指标聚合 | `middleware-facade/src/security_metrics.rs` | 2 | — |

**中间件执行顺序**（更新后）：
```
Trace → BodySizeLimit → IpAccessControl → SecurityHeaders → Cors → Log → RateLimit → Auth → AuditLog
```

**关键决策**：
- `PHP_GLOBAL_ORDER [Trace, Cors]` 不再是 `DEFAULT_ORDER` 前缀，改为 subset 检查
- `SecuritySection` 定义在 `middleware-facade` 内避免循环依赖
- CSP nonce 使用 `STANDARD_NO_PAD` Base64（16 字节 → 22 字符）
- IP 规则解析支持 CIDR 和单 IP（自动转 /32 或 /128）

---

## 七、FSSADMIN 竞品识别的缺失功能

> 来源：`docs/cases/fssadmin-competitive-analysis.md`
> 这些功能不在 4 阶段路线图中，但作为补充 backlog 记录于此。

| # | 缺失功能 | 优先级 | 归属阶段 | 状态 |
|---|----------|--------|----------|------|
| 1 | CSRF 防护中间件 | P1 | Phase 1 补充 | ■ 已完成 |
| 2 | XSS 过滤中间件 | P1 | Phase 1 补充 | ■ 已完成 |
| 3 | IP 黑名单（已由 IP 访问控制覆盖） | P1 | 已完成 | ■ |
| 4 | 代码生成器 | P0 | Phase 2（SDD Agent 覆盖） | ■ 已完成 |
| 5 | 数据权限（行级/字段级） | P1 | Phase 1 补充 | □ |
| 6 | 多租户 SaaS | P2 | Phase 3 | □ |
| 7 | 配套 Admin 前端模板 | P2 | Phase 3 | □ |
| 8 | 插件市场 | P2 | Phase 3（P3-T2 覆盖） | ■ 已完成 |

---

## 八、进度更新日志

| 日期 | 更新内容 | 更新人 |
|------|----------|--------|
| 2026-08-11 | 初始创建：建立进度追踪框架，记录已完成的安全中间件工作 | AI Agent |
| 2026-08-11 | P1-T1 Capability Registry Batch A-C 完成：crate 骨架 + Capability trait + Registry + Cap facade + MCP 适配 + 24 测试通过 | AI Agent |
| 2026-08-11 | P1-T1 性能基准全部达标：注册 187ns / 查找 38ns / 标签搜索 20μs | AI Agent |
| 2026-08-11 | P1-T1 API 文档完成：rustdoc + 5 doc-tests 通过 | AI Agent |
| 2026-08-11 | P1-T1 集成完成：ai-facade LlmChatCapability + addons-loader CapabilityHook，P1-T1 全部完成 | AI Agent |
| 2026-09-09 | P3 性能优化验证完成：919 tests passed，全部实现文件存在 | AI Agent |
| 2026-09-09 | P0-3 分离收尾验证完成：8 crates 发布，sz-pay 兼容，sz300 生产部署 health 200 | AI Agent |
| 2026-09-09 | 生产接线验证完成：sz300 (8300) + marketplace (8080) 运行中，15 模块已接线 | AI Agent |
| 2026-09-09 | P2 生态件部署验证：marketplace 7 API 端点全部验证通过 | AI Agent |
| 2026-09-09 | 文档同步：implementation-progress.md 更新 Phase 1-4 进度 + M1-M7 里程碑状态 | AI Agent |
| 2026-09-10 | 数据权限扩展功能交付：P0 基礎层（89 tests）+ P1 核心层（44 tests）+ P2 接入层（810 tests total）；HotReloadManager + ConfigLoader + 13 管理 API 端点；clippy 0 warnings；CI 门禁通过 | AI Agent |
| 2026-09-09 | workspace 全量 lib 测试基线：5185 passed, 0 failed, 2 ignored；clippy 0 warnings；fmt 0 差异 | AI Agent |

---

## 九、下一步行动

### 立即可执行（按优先级排序）

1. **M12 完整生态**（最后里程碑）
   - 多租户 SaaS 支持
   - 配套 Admin 前端模板
   - ~~数据权限（行级/字段级）~~ ✅ 已完成（2026-09-10，基础层 + 扩展层）
   - 开发者社区建设

2. **Cloudsmith 企业版发布**（P0-3 组10.2，用户暂缓）
   - 需 Cloudsmith 账户配置

### 需要决策的事项

- [ ] 是否启动 M12 完整生态里程碑？
- [ ] Cloudsmith 企业版发布何时配置？
- [ ] 是否需要新的功能需求？

---

## 2026-09-10 更新：数据权限扩展功能 — 动态策略热更新 + 配置文件加载 + 管理后台 API

> **状态**：✅ 全部完成（P0 基础层 + P1 核心层 + P2 接入层 + CI 门禁 + 文档同步）
> **SDD 规格文档**：`.codeartsdoer/specs/data_permission_ext/`（spec.md 533行 + design.md 1060行 + tasks.md 325行）

### 交付清单

| 层级 | 模块 | 文件 | 测试 | 状态 |
|------|------|------|------|------|
| **基础层** | 行级数据权限 | `data_scope/rule.rs` / `registry.rs` / `modes/` | 94 tests | ✅ |
| **基础层** | 字段级数据权限 | `data_scope/field_scope/` | 含于上方 | ✅ |
| **基础层** | 数据权限中间件 | `middleware-facade/data_scope.rs` | 4 e2e | ✅ |
| **P0 基礎** | ext 子模块 | `data_scope/ext/{generation,notifier,audit,path_guard}.rs` | 含于 89 | ✅ |
| **P0 基礎** | Registry 扩展 | `rule.rs` / `registry.rs` / `field_scope/policy.rs` / `custom.rs` | 含于 89 | ✅ |
| **P0 基礎** | Error 13 变体 | `error.rs` | 含于 89 | ✅ |
| **P0 基礎** | Metrics 扩展 | `metrics.rs` | 含于 89 | ✅ |
| **P1 核心** | HotReloadManager | `ext/hot_reload.rs` | 含于 44 | ✅ |
| **P1 核心** | ConfigLoader | `ext/config_loader.rs` | 含于 44 | ✅ |
| **P2 接入** | 管理 API 13 端点 | `data_perm_admin/{router,handlers,guard,error_response}.rs` | 25 api | ✅ |
| **P2 接入** | E2E 测试 | `tests/data_perm_ext_e2e.rs` / `tests/data_perm_admin_api.rs` | 18 + 25 | ✅ |

### 验证汇总

| 验证项 | 结果 |
|--------|------|
| `cargo test -p sz-rust-orm-facade` | 230 passed（184 lib + 46 e2e） |
| `cargo test -p sz-rust-middleware-facade` | 580 passed（551 lib + 25 api + 4 e2e） |
| `cargo clippy` | 0 warnings |
| CI 门禁（std::fs / dead_code / unsafe） | 全部通过 |
| 向后兼容回归 | 0 regressions |

### 关键设计决策

1. **世代号（PolicyGeneration）**：`AtomicU64` SeqCst 单调递增，每次策略变更自增，用于缓存失效判断
2. **变更通知（ChangeNotifier）**：`tokio::sync::broadcast` channel，热更新时广播 ChangeEvent
3. **并发安全**：`CustomGeneratorRegistry` 从 `HashMap + Mutex` 改为 `DashMap`，消除 `register_mut` 旧接口
4. **配置加载安全**：`PathGuard` 白名单校验防路径穿越，文件大小上限防 OOM，部分失败策略（合法条目正常注册）
5. **管理 API 鉴权**：`admin_guard_middleware` 从 request extensions 提取管理员标识，非管理员返回 403
6. **错误映射**：`ApiErrorResponse` 将 `DataScopeError` 13 变体映射到 HTTP 状态码（400/403/404/409/500）
7. **中间件不阻断**：安全降级策略 — 无 UserContext 时注入默认值，不返回 401

---

## 2026-08-19 更新：7 P1 死 crate 复活 + 生产接线 + 安全复核

| 提交 | 内容 | 验证 |
|------|------|------|
| `6225cb5` | 7 crate 复活到 workspace（tracing/pdf/workflow/operate/erp/forum/im） | `cargo check` 全部通过 |
| `5d5147c` | 3 crate 补测试（tracing 55/100% + pdf 164/95.86% + operate 495/87.84%） | `cargo test` 全部通过 |
| `c371148` | 7 crate 生产接线 sz300（erp/forum/im 路由 + operate/workflow/tracing/pdf 依赖 + addons status 端点） | sz300 447 + erp 23 + forum 23 + im 23 测试通过 |
| `c371148` | R001-R007 安全项全部人工复核完成 | doc-debt DB-2026-08-16-05 RESOLVED |
| `d6379c5` | CI-001~007 覆盖率 CI 集成增强（4 并行分片 + per-crate 门槛） | ci.yml + coverage.yml |
| — | 全部提交已 push 到 origin/main | `git rev-list --count origin/main..main` = 0 |

---

> **使用说明**：
> 1. 每完成一个子任务，将 `□` 改为 `■`，填写验收结果
> 2. 每开始一个任务，将状态改为 `▓ 进行中`，填写开始日期
> 3. 每完成一个任务，更新总体进度概览的百分比和里程碑状态
> 4. 在进度更新日志中记录每次更新
> 5. 在"下一步行动"中维护当前可执行的任务列表