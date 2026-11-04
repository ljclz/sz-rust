# 门禁阻断报告（2026-09-24）

- 分支 / commit：`main` @ `ff531ca`
- 范围：全量 15 门禁 + AI 评审，`origin/main..HEAD`（18 个未推送提交，219 文件，+32,576/−54；v1.4.0 批次：编译优化 + sz300 回归 workspace + i18n + WASM + gRPC 流式 + 新增 api-gateway/codegen-loop/config-center/distributed-tx/facade/ops-api 等 crate）
- 状态机：`scanning → compile ✗(环境) → static(fmt ✓/clippy ✗(环境)/unwrap ✗/sensitive ✗) → test ✗(环境) → integration ✗(环境) → ai ✓ → done`
- 事件：`ReviewBlocked`（blocking 7：critical ×4 / high ×2 / medium ×1 / low ×1）
- 审查报告（执行器产出）：docs/audit/2026-09-24-pr-review-main.md

## 根因 A（环境）：sccache 未安装 —— 4 个门禁本轮无代码信号

- **本批次引入**：`.cargo/config.toml`（v1.4.0 T01）新增 `rustc-wrapper = "sccache"`
- **本机缺失**：`which sccache` → NOT IN PATH；`sccache --version` → 不可执行
- **影响面**：check / clippy / test 三门禁的 critical 全是 `could not execute process 'sccache ...' (never executed)`；integration high 失败同根因。**本轮编译类门禁未产出任何代码缺陷信号**
- **配置自带的降级口**（config 注释）：`RUSTC_WRAPPER="" cargo build`
- **批次内部不一致**：`.githooks/pre-commit:7-9` 已做「sccache 缺失则禁用 wrapper」降级；但 **CI（ci.yml）无任何 sccache 安装/降级处理**（AI 评审第 1 条的增量发现，已独立核实）——CI 一旦实跑将以同方式失败

## 真实代码问题（门禁不依赖编译器独立取证）

### 1. [critical] 敏感字段暴露 ×2（铁律：敏感字段自动脱敏）

- `packages/sz-rust-sz300/src/config.rs:35` `DatabaseConfig.password`
- `packages/sz-rust-sz300/src/config.rs:50` `PgDatabaseConfig.password`
- 两结构体仅 derive `Deserialize`；按项目既有脱敏模式补 `#[serde(skip_serializing)]`（不引入 secrecy 新依赖）

### 2. [high] 生产代码裸 unwrap ×18（铁律 2）

分布：ai-facade 5（fallback.rs:145,148 / router.rs:140 等）、examples 4（wasm_*.rs 各 1）、sz300 3、ops-api 2、testkit 2、codegen-loop 1（parser.rs:159）、service-registry 1（gray_release.rs:114,119 / load_balancer.rs:153 / role_guard.rs:130）。清偿模式参照 DB-2026-08-16-04 先例（unwrap_or_else/expect 带 JD 上下文/锁中毒 into_inner）。

### 3. [medium] 空白错误

- `packages/sz-rust-sz300/scripts/concurrent_test.ps1:50` EOF 多余空行（AI 评审猜测「冲突标记残留」不实，实为 EOF 空行）

### 4. [low] 无断言测试 ×24（铁律 10/23）

集中在 addons-admin `tests/integration_test.rs`（测试名声称 returns_200/400/204 却零断言）与 testkit `http_client.rs`/`fixture.rs`（4 个）。覆盖率导向补测但无回归保护。

## AI 评审交叉核验（5 条，评分 4/10，deepseek-v4-flash）

| AI 意见 | 复核结论 |
|---|---|
| 1. sccache 致编译失败 + CI 未处理 | ✅ 实锤，且 **CI 缺口为门禁之外的增量有效发现**（ci.yml 零 sccache 处理，已核实） |
| 2. 18 处裸 unwrap | ✅ 实锤，与门禁一致 |
| 3. 敏感字段暴露 | ✅ 实锤，与门禁一致；处方（secrecy crate）不符合项目惯例，应走既有 `skip_serializing` 模式 |
| 4. 24 个无断言测试 | ✅ 实锤，与门禁一致 |
| 5. 空白/冲突标记 | ✅ 文件定位对，「冲突标记」猜测不实（EOF 空行） |

本轮 AI 可采纳性 5/5 定位准确（含 1 条增量发现），为 deepseek-v4-flash 观察以来最佳一轮；仅处方细节需按项目规范修正。

## 建议修复（按优先级）

1. **环境**：`cargo install sccache`（随批指南 docs/developer/sccache-guide.md §2.1，约 5-10 分钟）；CI ci.yml 增加安装或降级步骤（对齐 pre-commit:7-9 的处理）
2. **config.rs** 两个 password 字段补 `#[serde(skip_serializing)]`
3. **18 处 unwrap** 逐处清偿（expect 上下文 / `?` 传播 / 锁中毒 into_inner）
4. **24 个空洞测试** 补真断言（admin integration 测试应断言 status）或删除
5. `concurrent_test.ps1` 删 EOF 空行
6. 复跑：`bash scripts/audit/pr-review.sh --range origin/main..HEAD --ai --ai-timeout 300`（装 sccache 后编译类门禁才首次产出真实代码信号）

## 复跑要求

修复 1-5 后整轮重跑；sccache 安装后本轮 4 条环境型 critical 将转为真实编译/测试结果。

---

# 修复进展（2026-09-24 两轮修复后终态）

**阻塞 7 → 2**（0 critical / 1 high / 1 medium）。两轮修复均经真实门禁验证。

## 已修复并验证

| 项 | 修复 | 验证证据 |
|---|---|---|
| 根因 A：sccache 缺失（4 门禁无信号） | `cargo install sccache`（RUSTC_WRAPPER="" 逃生口安装）+ ci.yml 顶层 env 禁用 wrapper（CI 缓存由 rust-cache 承担，对齐 pre-commit 降级逻辑） | `cargo check --workspace --all-targets` 首次真实通过（26.7s）；clippy -D warnings 全绿（5.7s） |
| **新发现：sz300 两个 .rs 文件非法 UTF-8**（审查门禁无法捕获，rustc 必炸） | `middleware/role_guard.rs` + `controllers/admin.rs` 以企业版仓库干净文本为字符参照、保留本地批次 intentional 改动（Apache-2.0 头/strip_prefix 重构/同步 collect_server_info/去 2 处 tracing）整文件重建 | 两文件 `file` = 纯 UTF-8、0 U+FFFD；编译通过 |
| 敏感字段 ×2 | `config.rs` 两 password 字段 `#[serde(skip_serializing)]` | sensitive-audit：0 EXPOSED |
| [critical] redaction_test 2 失败 | 批次写了测试未写实现：两结构体改手工 `Debug`（密码字段 `[REDACTED]`，d14f9eb 先例），derive 移除 Debug | redaction_test 6/6 通过、config_test 3/3 通过 |
| [high] 裸 unwrap ×18 | role_guard 3 处随重建恢复 expect；其余 15 处逐处清偿（非空不变量→expect 带上下文、锁中毒→PoisonError::into_inner、静态 WAT/正则→expect） | check-unwrap：AUTHORITATIVE_PROD_UNWRAP=0 |
| [low] 空洞测试 ×24 | admin 集成测试 20 处 assert_status 断言内联进测试体（助手删除防 dead_code）；testkit 4 处补直接断言 | assertion-value 门禁 0 ERROR（仅 3 条历史 WARN） |
| [medium] ps1 EOF 空行 | 尾部空行删除（工作区已修） | `git diff --check` 工作区通过 |
| 执行器：集成目标名过时 | pr-review.sh `jobs_integration_test`→`db_integration_test`（批次改名所致，旧名调用必然失败） | 集成环节真实运行 |
| 执行器：集成环节无超时保护 | `timeout 600` + rc=124 判定（曾因测试挂死阻塞审查 50 分钟） | 终跑 20 分钟完整走完状态机 |

## 遗留阻塞（移交）

### 1. [high] `test_mysql_transaction_commit_rollback` 挂死 —— sz-orm 7.6.0 框架层回归

诊断已收口到框架层（sz-rust 侧无可修点）：
- 同文件 6 个非事务 MySQL 测试全过（连接池/CRUD/DDL/注入防护）
- **PG 事务测试 0.13s 通过**（同一 begin/commit/rollback API）
- MySQL 事务测试挂死零输出零 CPU；MySQL 无僵尸会话（processlist 仅 event_scheduler）
- 挂点在 sz-orm-sqlx 7.6.0 MySQL 路径 `begin_transaction` → `execute("BEGIN")`（any.rs:297）；7.7.0 未改 any.rs，升级不解决
- 处置归属：sz-orm 框架侧修复（框架源码不在本机）或用户裁定测试/门禁豁免

### 2. [medium] whitespace + AI 评审缓存

均锚定**已提交 diff**（origin/main..HEAD）：ps1 空行工作区已修、AI 意见为修复前缓存（不参与阻塞）。**提交本工作区后两项自然消除**，届时复跑应达 ReviewCompleted。
