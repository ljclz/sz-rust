# 门禁阻断报告（2026-09-13）

> **✅ 已解决（2026-09-13 同日）**：A1/B1/B2 修复于 `d1b0061`，C1/C2 守卫修复于 `ae27e57`；
> 复跑 `--range HEAD~2..HEAD` 结果 **0 critical / 0 high**（报告 `docs/audit/2026-09-13-pr-review-main.md`）。
> 唯一遗留：[medium] missing-key（AI_API_KEY 未配置，环境项，补 key 重跑 `--ai` 即消）。

- 分支 / commit：`main` @ `f6b0e7e`（审查范围 HEAD~1..HEAD，报告为时点快照）
- 执行器：`bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai`（两轮：首轮遇构建缓存损坏，清理 libsqlite3-sys 后重跑）
- 状态机：`scanning → compile → static → security → test → integration → ai → done`，最终 **done 但 BLOCKING = 5**（≥ medium 阈值）
- 详细报告：`docs/audit/2026-09-13-pr-review-main.md`

## 阻塞问题清单（按归因分类）

### A. 本次变更引入（f6b0e7e 新增代码）

| # | 严重度 | 门禁 | 证据 | 说明 |
|---|--------|------|------|------|
| A1 | high | 铁律 2 裸 unwrap | `packages/sz-rust-cli/src/cmd/admin.rs:224` | `let url = args.url.as_ref().unwrap();`。前置 `args.url.is_none()` 守卫（admin.rs:214-222）当前逻辑安全，但违反铁律 2 模式门禁，守卫重构后有真实 panic 风险 |

### B. 工作区既有（非本次范围引入，但同被全量门禁拦截）

| # | 严重度 | 门禁 | 证据 | 归因 |
|---|--------|------|------|------|
| B1 | critical | 敏感字段审计 | `packages/sz-rust-addons-admin/src/services/user_service.rs:17,24` | `CreateUserRequest.password` / `UpdateUserRequest.password` 无 `skip_serializing` 且派生 `Debug`（c96196e 引入，`{:?}` 日志可泄露明文密码） |
| B2 | high | 铁律 2 裸 unwrap | `packages/sz-rust-examples/src/bin/multi_tenant_demo.rs:75,120`（3 处） | multi-tenant 提交（2b149b6）引入的示例代码 |

### C. 门禁脚本与仓库结构漂移（1614e84 分离提交后未同步）

| # | 严重度 | 门禁 | 证据 | 说明 |
|---|--------|------|------|------|
| C1 | critical | G11 单元测试 | `cargo test -p sz-rust-sz300` → `error: package ID specification 'sz-rust-sz300' did not match any packages` | `1614e84`（2026-09-05，开源/企业版物理分离）已将 sz300 移出 workspace members（`git show 1614e84^:Cargo.toml` 含 1 处、`1614e84:Cargo.toml` 含 0 处），`pr-review.sh:243` 仍硬编码该包且无存在性守卫 |
| C2 | high | G12 集成测试 | `pr-review.sh:256` 同因 | `jobs_integration_test` 随 sz300 移出本仓库，门禁对象已不存在 |

### D. 环境降级（不阻塞判定链，如实记录）

- [medium] `missing-key`：`AI_API_KEY` / `CSDN_API_KEY` 在 shell、Machine/User 级环境变量均未设置（`powershell [Environment]::GetEnvironmentVariable(...)` 四项全 False），diff 缓存未命中（`22790eb983388daf` 不在 `~/.cache/sz-rust-review/`）。AI 评审跳过。补 key 后重跑 `--ai` 即可（diff hash 未变，缓存命中路径有效）。

## 已排除的假阳性

- 首轮 6 条 critical `compile-error`（libsqlite3-sys `bindgen.rs` 缺失）为构建缓存中断损坏（`F:\cargo-target\debug\build\libsqlite3-sys-*/out` 有 .o/.a 但缺 bindgen 生成物），`cargo clean -p libsqlite3-sys`（清除 247.1MiB）后重跑，G2 check / G3 fmt / G4 clippy 全部通过。**非代码问题，不计入阻断。**
- G3 fmt、feature-consistency 门禁本轮通过，无问题。

## 非阻塞债务（low，入 doc-debt 追踪）

- doc-code-consistency：128 处需核验声称（主要为 k8s-operator 存在于 packages/ 但非 member、历史文档引用已移除/虚构 crate 的回退标注）
- assertion-value：4 个无断言测试
- adr-code-consistency：1 处 ADR 漂移

## 修复建议（按优先级）

1. **A1**：`admin.rs:224` 改为 `let url = args.url.as_deref().ok_or_else(|| CliError::Args("--url is required".into()))?;`（或复用既有 CliError 构造），修复后重跑 G5。
2. **B1**：`user_service.rs:17,24` password 字段加 `#[serde(skip_serializing)]`；若 Debug 派生保留，建议手工实现 `Debug` 脱敏（与仓库 `#[serde(skip_serializing)]` 惯例一致）。
3. **C1/C2**：`pr-review.sh` 门禁 11/12（及 13/14 deep）对 `-p sz-rust-sz300` 增加存在性守卫（如 `cargo metadata` 查询或目录探测），sz300 测试职责移交企业版仓库审查流程；或从开源版门禁中显式移除并注释指向。
4. **D**：`export AI_API_KEY=...` 后重跑 `bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai` 补全 AI 评审环节。

## 复跑要求

A1/B1 修复并提交后，从 `static`（G5）起重跑；C1/C2 属脚本修复，修复后 G11/G12 应转为「跳过（守卫生效）」或移交企业版流程。
