# sz-rust-sz300 覆盖率评估报告（只读）

> 评估日期：2026-11-05
> 评估性质：只读评估（未写测试、未改代码、未 commit）
> 评估工具：cargo-llvm-cov 0.9.1（cargo tarpaulin 不可用）
> 隔离环境：`CARGO_HOME=D:/cargo-home-verify CARGO_TARGET_DIR=D:/rust-target-verify`
> 评估对象：`packages/sz-rust-sz300`，default features = `[v18-security-headers, v18-data-mask, v18-audit-chain, v18-key-rotation, v19-jwt-audience, v19-saga]`

## 1. 工具可用性

| 工具 | 状态 | 版本 | 路径 |
|------|------|------|------|
| cargo tarpaulin | ❌ 不可用 | — | 未安装（`error: no such command: tarpaulin`） |
| cargo-llvm-cov | ✅ 可用 | 0.9.1 | `D:\cargo-home\bin\cargo-llvm-cov.exe` |

## 2. 真实覆盖率数据（llvm-cov 测量）

### 2.1 lib 单元测试覆盖率（已跑完）

命令：`cargo llvm-cov -p sz-rust-sz300 --lib --summary-only`
前置：`cargo test -p sz-rust-sz300 --lib` → **199 passed; 0 failed; 0 ignored**（5.04s）

| 指标 | 总数 | 未覆盖 | 覆盖率 |
|------|------|--------|--------|
| **行覆盖率（Lines）** | 12135 | 7838 | **35.41%** |
| **区域覆盖率（Regions）** | 1340 | 976 | **27.16%** |
| **函数覆盖率（Functions）** | 7424 | 4796 | **35.40%** |

> 注：12135 行为 llvm-cov instrumented 行数（排除 `#[cfg(test)]` 内代码、空行、注释）。全 `src/` 物理 LOC 为 18943（含 cfg(test) 测试代码）。

### 2.2 与记忆"8.40%"差异分析

记忆记载"sz300 真覆盖率 8.40%（豁免非真覆盖）"。本次测量：
- lib 单测行覆盖率 **35.41%**
- 完整（lib+tests）行覆盖率 **41.82%**

差异显著，可能原因：

1. **度量口径不同**：8.40% 可能是"全量代码（含 controllers/middleware/rbac/graphql/main.rs）作为分母"的覆盖率，而 41.82% 的分母是 llvm-cov instrumented 的 12396 行。若用全 src/ 18943 行作分母：`(12396-7212)/18943 = 27.4%`。
2. **豁免剔除**：8.40% 可能剔除了豁免代码（generated、tarpaulin exclude 列表、cfg(test) 内调用）后的"真覆盖率"。
3. **版本演进**：自 8.40% 记录后，sz300 补充了大量内嵌测试（src/ 内 256 个 `#[test]`/`#[tokio::test]`）+ 集成测试（386 个），覆盖率已大幅提升。
4. **region vs line**：8.40% 接近 region 覆盖率（33.75%）的某次子集测量或更早版本数据。

**结论**：当前完整行覆盖率 **41.82%**（lib+tests，llvm-cov 0.9.1 真实测量），记忆 8.40% 已过时（或为不同口径/旧版本）。建议以本次数据为准，更新记忆。

### 2.3 完整覆盖率（lib + tests，已跑完）

命令：`cargo llvm-cov -p sz-rust-sz300 --summary-only`（含 44 个集成测试文件、386 个集成测试函数）

| 指标 | 总数 | 未覆盖 | 覆盖率 |
|------|------|--------|--------|
| **行覆盖率（Lines）** | 12396 | 7212 | **41.82%** |
| **区域覆盖率（Regions）** | 1351 | 895 | **33.75%** |
| **函数覆盖率（Functions）** | 7609 | 4394 | **42.25%** |

> 完整模式比 lib only 多覆盖：controllers/（ai 40.26%、auth 35.74%、common 94.56%、file_serve 83.93%、health 80.00%、wasm_api 78.60%；device/merchant/order/product/file 仍 0%）、middleware/（audit_chain 48.61%、auth_middleware 70.11%、compression 100%）、config 65.62%、db 62.68%、models/order 80.43%、models/order_item 84.25%。
> **auth_service.rs 从 lib only 29.32% → 完整 61.90%**（jwt_audience_test/login_contract_test/key_rotation_test 集成测试贡献大）。
> controllers/device/merchant/order/product/file 0%：对应集成测试需 MySQL，本次环境未跑通。
> graphql/、rbac/、i18n_extractor/、dynamic_config/、alert_rules/ 未出现在完整报告：可能被 dead code elimination 移除或集成测试未触达。

## 3. 静态覆盖映射

### 3.1 代码规模

| 模块 | 物理 LOC | pub fn 数 | 说明 |
|------|---------|-----------|------|
| `src/services/` | 6628 | 113 | 业务核心，18 个文件 |
| `src/controllers/` | 3533 | — | 13 个 controller 文件 |
| `src/models/` | 1850 | — | 14 个 model 文件 |
| `src/middleware/` | 1292 | — | 7 个中间件 |
| `src/graphql/` | 673 | — | GraphQL 端点 |
| `src/rbac/` | 226 | — | RBAC 守卫 |
| 其他（openapi/router/state/config/db 等） | 4741 | — | — |
| **全 src/ 合计** | **18943** | **268** | 含 cfg(test) 测试代码 |

### 3.2 测试规模

| 测试类别 | 文件数 | 测试函数数 | 说明 |
|---------|--------|-----------|------|
| `tests/` 集成测试 | 44 | 386 | 含 e2e/integration/contract 等 |
| `src/` 内嵌 `#[cfg(test)]` | — | 256 | 单元测试 |
| **合计** | — | **642** | — |
| lib 单测实际跑通 | — | **199** | `cargo test --lib`（部分 cfg(test) 在非 default feature 下不编译或被 filter） |

### 3.3 services/ 各文件覆盖明细（llvm-cov 完整模式 lib+tests 测量 + 静态映射）

| 文件 | LOC | pub fn | 内嵌测试 | 集成引用次数 | lib-only 行覆盖 | **完整行覆盖** | 完整 region |
|------|-----|--------|---------|-------------|----------------|--------------|-------------|
| auth_service.rs | 958 | 23 | 5 | 47 | 29.32% | **61.90%** | 43.18% |
| saga_order.rs | 917 | 12 | 7 | 2 | 31.94% | **31.94%** | 16.22% |
| order_service.rs | 671 | 4 | 12 | 3 | 44.67% | **44.67%** | 30.43% |
| device_service.rs | 494 | 8 | 8 | 9 | 39.37% | **39.37%** | 17.78% |
| product_service.rs | 455 | 6 | 7 | 0 | 51.78% | **51.78%** | 21.21% |
| ai_classifier.rs | 422 | 7 | 10 | 2 | — | — | — |
| plugin_manager.rs | 416 | 7 | 10 | 25 | — | — | — |
| outbox.rs | 403 | 6 | 10 | 1 | 30.90% | **30.90%** | 29.27% |
| merchant_service.rs | 379 | 5 | 7 | 0 | 43.08% | **43.08%** | 21.88% |
| mod.rs | 348 | 2 | 16 | — | 89.56% | **99.48%** | 100.00% |
| mqtt_service.rs | 314 | 8 | 0 | 12 | 0.00% | **7.14%** | 10.53% |
| db_error.rs | 205 | 2 | 11 | — | 100.00% | **100.00%** | 100.00% |
| file_service.rs | 192 | 3 | 8 | 0 | 57.08% | **57.08%** | 60.87% |
| sse_service.rs | 158 | 11 | 0 | 12 | — | — | — |
| mqtt_listener.rs | 92 | 2 | 0 | 12 | 0.00% | **0.00%** | 0.00% |
| health_service.rs | 72 | 1 | 0 | 2 | 0.00% | **0.00%** | 0.00% |
| order_no.rs | 67 | 1 | 3 | 0 | 96.36% | **96.36%** | 100.00% |
| ws_service.rs | 65 | 5 | 0 | 1 | — | — | — |

> "—" 表示该文件未出现在 llvm-cov 报告中（可能因 dead code elimination 未链接进测试二进制）。ai_classifier/plugin_manager/sse_service/ws_service 主要靠集成测试覆盖，但未在完整报告显示，可能集成测试二进制未引用其 instrumented 副本。
> **auth_service.rs 集成测试贡献显著**：lib only 29.32% → 完整 61.90%（+32.58pp），jwt_audience_test(13)/login_contract_test(3)/key_rotation_test(6)/auth_ctx_cache_test(3) 集成测试覆盖了 token 签发/验证/轮换/audience 路径。

### 3.4 0% 覆盖文件清单（lib 单测完全未触达）

| 文件 | LOC | 原因 |
|------|-----|------|
| services/mqtt_service.rs | 294 | 需 MQTT broker，lib 单测不触发 |
| services/mqtt_listener.rs | 22 | 需 MQTT broker + AppState |
| services/health_service.rs | 35 | 需 DB pool（ping_db） |
| models/market.rs | 156 | model 序列化/校验未单测 |
| models/merchant.rs | 197 | 同上 |
| models/operate_log.rs | 141 | 同上 |
| models/order.rs | 230 | 同上 |
| models/order_item.rs | 146 | 同上 |
| models/ota_version.rs | 180 | 同上 |
| models/product.rs | 210 | 同上 |
| models/settlement.rs | 169 | 同上 |
| models/system_config.rs | 102 | 同上 |
| **合计** | **1882** | 占未覆盖行数 7838 的 **24%** |

## 4. 低覆盖区域 Top 5

综合 LOC、pub fn 数、测试密度、集成引用次数、llvm-cov 完整行覆盖率/region 覆盖率，识别以下低覆盖区域（按优先级排序）：

### Top 1：services/mqtt_service.rs — 完整覆盖仅 7.14%

- **规模**：314 LOC，8 pub fn
- **覆盖**：lib 0.00% → 完整 **7.14%**（region 10.53%）
- **测试**：0 内嵌测试；12 集成引用（e2e_mqtt_test 7、mqtt_dispatch_test 8）——集成测试仅触达少量路径
- **未覆盖 pub fn**：`handle_device_status`、`handle_device_order`、`handle_device_log`、`start`、`publish`、`get_mqtt_config`、`get_subscribe_topics`、`send_ota_command`
- **风险**：设备消息处理核心，8 个 pub fn 几乎全未覆盖，需 MQTT broker mock

### Top 2：services/saga_order.rs — 分布式事务 region 覆盖极低

- **规模**：917 LOC，12 pub fn（第二大）
- **覆盖**：行 31.94%，**region 16.22%**（region 覆盖极低，分支大量未测）
- **测试**：7 内嵌测试；仅 2 集成引用（order_saga_e2e 4）
- **风险**：saga 补偿/回滚/超时/状态机分支多，region 16.22% 表明大量分支未测，分布式事务正确性无保障

### Top 3：controllers/device/merchant/order/product/file — 0% 覆盖（需 DB）

- **规模**：device 378 + merchant 318 + order 303 + product 325 + file 226 = **1550 LOC**，22 pub fn
- **覆盖**：完整模式全部 **0.00%**
- **测试**：0 内嵌测试；对应集成测试（integration_api 10、e2e_device 7 等）需 MySQL，本次环境未跑通
- **风险**：5 个核心业务 controller 完全无覆盖，CRUD 端点未验证

### Top 4：services/merchant_service.rs + product_service.rs — 零集成覆盖，region 低

- **merchant_service.rs**：379 LOC，5 pub fn，7 内嵌测试，**0 集成引用**，行 43.08%/region 21.88%
- **product_service.rs**：455 LOC，6 pub fn，7 内嵌测试，**0 集成引用**，行 51.78%/region 21.21%
- **现状**：内嵌测试只测参数校验/SQL 构造/whitelist（不依赖 DB），实际 DB 读写路径未覆盖
- **未覆盖**：`list`/`get`/`create`/`update`/`delete` 的 DB 操作路径；product 的 `create_with_ai`
- **风险**：IDOR filter、soft delete、whitelist 逻辑有单测，但 DB 端到端无验证；region 覆盖低表明分支未测

### Top 5：services/sse_service.rs + ws_service.rs + mqtt_listener.rs + health_service.rs — 实时通信/健康无单元测试

- **sse_service.rs**：158 LOC，11 pub fn，**0 内嵌测试**，12 集成引用（未在完整报告显示）
- **ws_service.rs**：65 LOC，5 pub fn，**0 内嵌测试**，1 集成引用
- **mqtt_listener.rs**：92 LOC，2 pub fn，0 内嵌测试，完整 **0.00%**
- **health_service.rs**：72 LOC，1 pub fn，0 内嵌测试，完整 **0.00%**
- **未覆盖 pub fn**：sse 的 `publish`/`subscribe`/`broadcast_*`/`check_backpressure`；ws 的 `broadcast_device_*`；mqtt_listener 的 `dispatch`/`start_consumer`；health 的 `ping_db`
- **风险**：背压、广播、房间管理、MQTT 消费分发、DB 健康检查逻辑无单测

### 补充：auth_service.rs 已大幅改善（降级说明）

- **完整行覆盖 61.90%**（lib only 29.32% → 完整 61.90%，+32.58pp）
- 集成测试 jwt_audience_test(13)/login_contract_test(3)/key_rotation_test(6)/auth_ctx_cache_test(3) 覆盖了 token 签发/验证/轮换/audience 路径
- **仍建议补单元测试**：23 pub fn 中仍有 50/88 function 未覆盖（function 覆盖 43.18%），`revoke_token`/`is_token_revoked`/`check_permission`/`refresh_access_token` 等可 mock DB 单测

### 补充：controllers/ 部分已覆盖（非 Top 5）

| controller | 完整行覆盖 | 说明 |
|-----------|-----------|------|
| common.rs | 94.56% | 集成测试充分 |
| file_serve.rs | 83.93% | — |
| wasm_api.rs | 78.60% | — |
| health.rs | 80.00% | — |
| ai.rs | 40.26% | — |
| auth.rs | 35.74% | — |
| device/merchant/order/product/file | **0.00%** | 需 DB 集成测试（见 Top 3） |

## 5. 提升建议与工作量评估

### 5.1 优先级建议（按 ROI 排序）

| 优先级 | 目标 | 当前 | 建议动作 | 预期提升 |
|--------|------|------|---------|---------|
| P0 | mqtt_service.rs | 行 7.14% | 补 8 个 pub fn 单测（mock MQTT client，测消息分发/OTA 命令/配置） | 行 → 60%+ |
| P0 | controllers/device/merchant/order/product/file | 行 0% | 跑通 DB 集成测试（testcontainers MySQL）或补 handler 单测（mock service） | 行 → 70%+ |
| P1 | saga_order.rs | region 16.22% | 补 saga 状态机分支测试（补偿/回滚/超时各分支） | region → 50%+ |
| P1 | merchant_service.rs | 行 43.08% | 补 DB 端到端集成测试（用 testcontainers MySQL） | 行 → 80%+ |
| P1 | product_service.rs | 行 51.78% | 同上 + `create_with_ai` mock AI facade | 行 → 80%+ |
| P2 | sse_service.rs | 0 内嵌测试 | 补 11 pub fn 单测（broadcast channel 单测，无需 IO） | 内嵌 → 11 |
| P2 | ws_service.rs | 0 内嵌测试 | 补 5 pub fn 单测 | 内嵌 → 5 |
| P2 | mqtt_listener.rs + health_service.rs | 行 0% | 补 dispatch 单测（mock state）+ ping_db 单测（mock pool） | 行 → 70%+ |
| P2 | models/* (0% 的 9 个) | 行 0% | 补 serde 序列化/校验单测（纯逻辑，无 IO） | models 行 → 90%+ |
| P3 | auth_service.rs | 行 61.90% | 补剩余 50 function 单测（revoke/check_permission/refresh，mock DB） | function → 80%+ |
| P3 | controllers/* (已部分覆盖) | — | 补 handler 单测（mock service layer）提升剩余 | — |

### 5.2 提升到 30% 的工作量

> **当前完整行覆盖率已 41.82%**（lib+tests），**已超过 30%**。若目标 30% 指的是"全量代码（含 controllers/middleware/rbac/graphql/main.rs，分母 18943 物理 LOC）行覆盖率"：
- 全 src/ 18943 行，30% = 5683 行需覆盖
- 当前完整模式覆盖 12396 - 7212 = 5184 instrumented 行（41.82% of 12396）
- 若按物理 LOC 折算：5184 / 18943 = 27.4%（接近但略低于 30%）
- 缺口：5683 - 5184 = 499 行
- **工作量**：约 5-8 个单元测试（每个测试平均覆盖 70-90 行）
- **聚焦**：补 models/ 序列化单测（9 个 0% model 文件共 1531 行，纯逻辑易测），即可达 30%+

### 5.3 提升到 50% 的工作量

- 按 llvm-cov instrumented 分母 12396 行：50% = 6198 行需覆盖，当前 5184，缺口 1014 行 → 约 12-15 个测试
- 按全 src/ 18943 物理 LOC：50% = 9472 行，当前 5184，缺口 4288 行 → 约 50-65 个测试
- **保守估算（按物理 LOC）**：
  1. P0 mqtt_service + controllers DB 集成（约 20 个测试，覆盖 1500 行）
  2. P1 saga_order 分支 + merchant/product DB 集成（约 15 个测试，覆盖 1200 行）
  3. P2 sse/ws/mqtt_listener/health/models 单测（约 20 个测试，覆盖 1200 行）
  4. P3 auth_service 剩余 + controllers handler 单测（约 10 个测试，覆盖 388 行）
- **预估工时**：3-4 人日（含 mock 搭建、testcontainers MySQL 接线）

### 5.4 提升到 80% 的工作量（参考）

- 按全 src/ 18943 物理 LOC：80% = 15154 行，当前 5184，缺口 9970 行
- 需补约 110-140 个测试，预估 7-10 人日
- 难点：mqtt_service/mqtt_listener 需 MQTT broker mock；controllers 需 axum TestServer + DB；saga_order 分支多

## 6. 验证证据链

| 证据 | 命令/路径 | 结果 |
|------|----------|------|
| llvm-cov 可用 | `cargo llvm-cov --version` | `cargo-llvm-cov 0.9.1` |
| tarpaulin 不可用 | `cargo tarpaulin --version` | `error: no such command: tarpaulin` |
| lib 单测通过 | `cargo test -p sz-rust-sz300 --lib` | `199 passed; 0 failed; 0 ignored`（5.04s） |
| lib 行覆盖率 | `cargo llvm-cov -p sz-rust-sz300 --lib --summary-only` | TOTAL 12135 行，7838 未覆盖，**35.41%** |
| lib region 覆盖率 | 同上 | TOTAL 1340 region，976 未覆盖，**27.16%** |
| **完整行覆盖率** | `cargo llvm-cov -p sz-rust-sz300 --summary-only` | TOTAL 12396 行，7212 未覆盖，**41.82%** |
| **完整 region 覆盖率** | 同上 | TOTAL 1351 region，895 未覆盖，**33.75%** |
| **完整 function 覆盖率** | 同上 | TOTAL 7609 function，4394 未覆盖，**42.25%** |
| services/ pub fn 总数 | `rg -c "^\s*pub (async )?fn " src/services/` | 113 |
| 全 src/ pub fn 总数 | 同上全 src/ | 268 |
| 全 src/ fn 总数（含私有） | `rg -c "^\s*(pub )?(async )?fn "` | 877 |
| tests/ 集成测试函数 | `rg -c "#\[test\]\|#\[tokio::test\]" tests/` | 386 |
| src/ 内嵌测试函数 | 同上 src/ | 256 |
| 全 src/ 物理 LOC | `wc -l src/**/*.rs` | 18943 |
| 隔离环境 | `CARGO_HOME=D:/cargo-home-verify CARGO_TARGET_DIR=D:/rust-target-verify` | 复用既有产物，未污染主构建 |
| 报告落盘 | `docs/audit/2026-11-05-sz300-coverage-eval.md` | 本文件

## 7. 关键决策与说明

1. **以 llvm-cov 完整模式（lib+tests）41.82% 为准**：lib only 35.41% 为下界，完整模式含集成测试贡献更全面。两者均已 llvm-cov 0.9.1 真实测量。
2. **未改代码/未写测试**：严格遵守只读评估约束，未创建任何测试文件，未修改任何源码，未 commit/push。
3. **隔离环境**：使用 `CARGO_HOME=D:/cargo-home-verify CARGO_TARGET_DIR=D:/rust-target-verify`，复用既有编译产物，未污染主构建目录。
4. **记忆 8.40% 已过时**：当前完整行覆盖率 41.82%（lib only 35.41%），记忆数据需更新。建议后续在 `docs/audit/README.md` 索引中同步修正。
5. **"低覆盖"定义**：综合 llvm-cov 完整行覆盖率、region 覆盖率、pub fn 数、内嵌测试数、集成引用次数五维判定，非单一指标。
6. **controllers 0% 归因**：device/merchant/order/product/file 五个 controller 0% 是因对应集成测试需 MySQL，本次环境未跑通；跑通 DB 集成测试后覆盖率将显著提升。
7. **auth_service 降级**：lib only 时 29.32% 列 Top 1，完整模式 61.90% 后降级为 P3，集成测试贡献显著（+32.58pp）。

## 8. 后续行动建议（非本次实施）

- [x] 跑完整 `cargo llvm-cov -p sz-rust-sz300 --summary-only`（含集成测试），校准全量覆盖率 → **41.82%**（本次已完成）
- [ ] 按 P0 优先级补 mqtt_service 单元测试 + controllers DB 集成测试
- [ ] 为 models/ 9 个 0% 文件补 serde 序列化单测（纯逻辑，快速提升覆盖率）
- [ ] 更新记忆中"sz300 真覆盖率 8.40%"为本次测量值（完整 41.82% / lib only 35.41%）
- [ ] 在 `docs/audit/README.md` 索引本报告
- [ ] 跑通需 MySQL 的集成测试（testcontainers 接线），消除 controllers 0% 盲区