---
name: sz-rust-pr-review
description: PR/提交审查编排 — diff 扫描 → 静态检查 → 安全门禁 → 严重度汇总报告（状态机）
tools: [git, cargo-clippy, node, bash]
agentMode: auto
---

# PR 审查编排（sz-rust-pr-review）

审查一次提交/PR 变更集，串起项目全部既有检查资产，输出带状态机与严重度模型的汇总报告。

## 触发条件

- 提交前/合入前对变更集做全量质量审查
- 需要"一次跑完所有门禁并生成报告"时（替代逐条手动执行）

## 执行

```bash
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --severity-threshold medium
```

参数：
- `--range <git-range>`：审查范围（默认 `HEAD~1..HEAD`；PR 场景用 `main...HEAD`）
- `--severity-threshold <low|medium|high|critical>`：阻塞阈值（默认 medium，≥ 阈值的问题导致审查失败、禁止合入）
- `--report <path>`：报告输出路径（默认 `docs/audit/<date>-pr-review-<branch>.md`）

## 状态机

```
scanning → static → security → done / failed
```

任一步骤命令失败即 `failed` 并退出非零（fail-closed）；报告记录完整状态流转。

## 环节与严重度映射

| 环节 | 检查 | 严重度映射 |
|------|------|-----------|
| diff 扫描 | `git diff --check`（空白/冲突标记） | whitespace-error → medium |
| 静态 | `cargo fmt --all --check` | fmt → medium |
| 静态 | `cargo clippy --workspace --all-targets -D warnings` | compile-error → critical / lint-warning → medium |
| 安全 | `sensitive-field-audit.js` | EXPOSED → critical |
| 一致性 | `doc-code-consistency.js` / `adr-code-consistency.js` / `assertion-value-check.js` | 失败 → low（不阻塞） |
| 一致性 | `feature-consistency.js` | 失败 → high |

## 通过标准

- 状态机到达 `done` 且无 ≥ threshold 级别问题 → 退出码 0，报告 ✅ 通过
- 任一步骤失败或存在 ≥ threshold 问题 → 退出码非零，报告 ❌ 阻塞/失败
- 报告落盘 `docs/audit/`，含状态流转、问题清单（severity/file/rule/message）、结论

## 已知边界

- AI 评审环节（LLM 评审 diff）为后续项：依赖 `sz-rust-ai-facade` 的真实 Provider 实现（当前仅有 trait 定义，未接线）
- 只审查静态资产，不运行测试（测试验证走 `sz-rust-test-coverage` / gauntlet）
