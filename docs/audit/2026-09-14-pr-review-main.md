# PR 审查报告（2026-09-14，branch: main，range: HEAD~4..HEAD）

> 审查时点: `HEAD @ f0375a0`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 .cargo/mutants.toml                                |    4 -
 README.en.md                                       |    2 +-
 README.md                                          |    2 +-
 ...274\200\345\217\221\345\217\202\347\205\247.md" |  115 +
 docs/audit/2026-09-14-coverage-baseline-full.txt   | 7822 ++++++++++++++++++++
 ...211\247\350\241\214\346\212\245\345\221\212.md" |   51 +
 docs/audit/doc-debt.md                             |    4 +-
 packages/sz-rust-cli/tests/plugin_behavior.rs      |   15 +
 packages/sz-rust-k8s-operator/Cargo.toml           |   23 -
 packages/sz-rust-k8s-operator/src/crd.rs           |  136 -
 packages/sz-rust-k8s-operator/src/lib.rs           |   25 -
 packages/sz-rust-k8s-operator/src/reconcile.rs     |  472 --
 scripts/audit/adr-code-consistency.js              |   13 +
 scripts/audit/doc-code-consistency.js              |    4 +-
 14 files changed, 8023 insertions(+), 665 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

# PR 评审报告：sz-rust 项目

## 评审范围

本次评审聚焦于 PR 中涉及的核心变更：`.cargo/mutants.toml` 移除 `pay.rs` 排除项、README 文档更新、以及新增的项目状态审计文档。

---

## 一、最重要的潜在问题

### 1. 【高】`pay.rs` 变异测试排除移除缺乏验证依据（可维护性/质量风险）

**问题描述**：`.cargo/mutants.toml` 中移除了 `"**/pay.rs"` 的排除项，但 FIXME 注释中明确要求"补齐签名验签/金额计算单测后必须移出（退出条件：pay.rs 行覆盖 ≥75%）"。PR 中未提供任何证据表明该退出条件已满足。

**风险**：
- 若 `pay.rs` 行覆盖率未达 75%，移除排除项将导致变异测试产生大量存活变异体，CI 可能失败或产生噪音
- 更严重的是，支付逻辑的变异体存活意味着**资金计算错误可能未被测试捕获**，这是资金安全级别的缺陷

**修改建议**：

```toml
# .cargo/mutants.toml
exclude_globs = [
    # 支付聚合层：变异测试运行时长超预算临时排除。
    # FIXME(DB-2026-09-03-01): 资金逻辑变异体存活代价最高，
    # 补齐签名验签/金额计算单测后必须移出（退出条件：pay.rs 行覆盖 ≥75%）
    # 2026-09-13 移除前需验证：cargo llvm-cov --package sz-rust-core -- pay.rs 行覆盖 ≥75%
    # "**/pay.rs",  # 暂不移除，等待覆盖率验证
    "**/qr_code.rs",
]
```

**或者**（若确认覆盖率已达标），在 PR 描述中附上验证数据：

```bash
# 需在 PR 描述中提供：
cargo llvm-cov --package sz-rust-core -- --show-line-coverage src/pay.rs | tail -5
# 预期输出：覆盖率 ≥75%
```

---

### 2. 【高】README 中 ADR 引用编号不一致（可维护性/文档一致性）

**问题描述**：`README.md` 中引用 `ADR-0021`，而 `README.en.md` 和新增审计文档中引用 `ADR-038`。同一变更在两个文件中引用了不同的 ADR 编号。

**风险**：
- 文档使用者无法确定哪个 ADR 是权威引用
- 若 ADR 编号体系存在迁移（如从 0021 重编号为 038），可能导致追溯困难

**修改建议**：

```markdown
<!-- README.md -->
- **K8s Operator**：（⚠️ 已移除：`sz-rust-k8s-operator` 孤儿 crate，无消费者，22 测试保留在 git 历史，详见 ADR-038（原 ADR-0021 已重编号））
```

---

### 3. 【中】新增审计文档中 Oracle WIP 未提交变更的风险提示（并发/流程风险）

**问题描述**：审计文档明确提到"工作区存在未提交 Oracle 接入 WIP（`Cargo.toml` sz-orm 6.2.0→6.9.0、`sz-rust-cli` 的 admin/migrate/cli 及 CHANGELOG.md，另有 12 个 test_oracle_*.js 未跟踪脚本）——合入时必须走完整审查流程"。

**风险**：
- 这些未提交变更与当前 PR 的提交历史交织，若后续合入顺序不当，可能导致依赖冲突
- 12 个未跟踪的 `test_oracle_*.js` 脚本可能包含敏感信息（如数据库连接串）

**修改建议**：在 PR 描述中明确声明：

```markdown
## 合入前检查清单
- [ ] 确认 Oracle WIP 变更（sz-orm 6.9.0、sz-rust-cli 修改）已提交或明确排除在本 PR 范围外
- [ ] 确认 12 个 test_oracle_*.js 未跟踪脚本已加入 .gitignore 或已清理
- [ ] 确认 Cargo.toml 中 sz-orm 版本锁定为 6.2.0（本 PR 不涉及升级）
```

---

### 4. 【中】README 功能声明与代码实现的一致性风险（可维护性）

**问题描述**：README 中大量使用"✅ 生产已接入"标记，但审计文档显示"release/coverage CI 自 09-05 起实际是坏的，本次才修复"。这意味着 README 中的功能声明可能超前于实际可验证状态。

**风险**：
- 新开发者依据 README 判断功能可用性，可能被误导
- 若 CI 在 PR 合入前未完整运行，功能声明可能不准确

**修改建议**：在 README 中增加验证状态说明：

```markdown
> **验证状态说明**：所有"✅ 生产已接入"标记均基于 2026-09-13 审计快照（main @ 52bb534）。
> CI 状态：release/coverage 已于 09-13 修复，当前 main 分支 CI 通过。
> 后续功能变更需同步更新本说明。
```

---

### 5. 【低】审计文档中"待重测"项未闭环（可维护性）

**问题描述**：审计文档中标注"所有数字均附来源，未实测项显式标注「待重测」"，但未提供待重测项的完整清单和跟踪机制。

**风险**：
- 待重测项可能被遗忘，导致文档中的数字过时
- 缺乏跟踪机制，无法确认哪些项已重测、哪些仍待验证

**修改建议**：在文档末尾增加跟踪表：

```markdown
## 待重测项跟踪

| 项目 | 标注日期 | 重测状态 | 重测日期 | 结果 |
|------|----------|----------|----------|------|
| [待补充] | 2026-09-13 | ⏳ 待重测 | - | - |
| [待补充] | 2026-09-13 | ⏳ 待重测 | - | - |

> 重测完成后更新此表，并在文档头部更新"时点锚定"。
```

---

## 二、整体评分

**评分：6.5 / 10**

**评分依据**：

| 维度 | 得分 | 说明 |
|------|------|------|
| 正确性 | 7/10 | 文档更新准确，但 ADR 编号不一致 |
| 安全性 | 6/10 | `pay.rs` 变异测试排除移除缺乏验证，资金逻辑风险 |
| 可维护性 | 6/10 | 文档结构清晰，但 ADR 引用不一致、待重测项无跟踪 |
| 并发/流程 | 7/10 | 审计文档明确标注 WIP 风险，但未提供合入策略 |
| 完整性 | 7/10 | 审计文档内容详实，但缺少验证数据支撑关键决策 |

**主要扣分项**：
1. `pay.rs` 排除移除未附覆盖率验证数据（-1.5）
2. ADR 编号不一致（-1.0）
3. Oracle WIP 未提交变更的合入策略不明确（-1.0）

**建议**：在合入前补充 `pay.rs` 覆盖率验证数据，统一 ADR 引用，并明确 Oracle WIP 的处理策略。


## 结论
✅ 通过（无 ≥ medium 级别问题）
