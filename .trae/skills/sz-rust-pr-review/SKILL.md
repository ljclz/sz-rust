---
name: sz-rust-pr-review
description: PR/提交审查编排 — diff 扫描 → 静态检查 → 安全门禁 → 严重度汇总报告（状态机）
tools: [git, cargo-clippy, node, bash]
agentMode: auto
---

# PR 审查编排（sz-rust-pr-review）

审查一次提交/PR 变更集，串起项目全部既有检查资产，输出带状态机与严重度模型的汇总报告；可选 AI 评审环节（CSDN 大模型）。

## 触发条件

- 提交前/合入前对变更集做全量质量审查
- 需要"一次跑完所有门禁并生成报告"时（替代逐条手动执行）
- 需要 LLM 对变更集做人工级评审时（加 `--ai`）

## 执行

```bash
# 静态审查
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --severity-threshold medium

# 静态 + AI 评审（OpenAI 兼容 Provider，默认 CSDN）
export AI_API_KEY=xxx   # 兼容旧变量名 CSDN_API_KEY
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai

# 切换 Provider 示例（快手 KAT-Coder-Pro-V2.5）
export AI_API_KEY=fPJ... AI_BASE_URL=https://wanqing.streamlakeapi.com/api/gateway/coding/v1 AI_MODEL=KAT-Coder-Pro-V2.5
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai
```

参数：
- `--range <git-range>`：审查范围（默认 `HEAD~1..HEAD`；PR 场景用 `main...HEAD`）
- `--severity-threshold <low|medium|high|critical>`：阻塞阈值（默认 medium，≥ 阈值的问题导致审查失败、禁止合入）
- `--ai`：启用 AI 评审环节（OpenAI 兼容端点。默认 CSDN：`https://ai.csdn.net/api/model/v1` + `model=glm_for_coding` 套餐；可用 `AI_API_KEY` / `AI_BASE_URL` / `AI_MODEL` 三个环境变量切换任意 OpenAI 兼容 Provider，如快手 `https://wanqing.streamlakeapi.com/api/gateway/coding/v1` + `KAT-Coder-Pro-V2.5`；`CSDN_API_KEY` 为旧变量名兼容）
- `--deep`：追加深验证（变异测试 + 变更行覆盖率，耗时 10+ 分钟，需 cargo-mutants / cargo-llvm-cov）
- `--skip-integration`：跳过真实集成测试环节（本机无 MySQL 时使用，如实记录跳过）
- `--report <path>`：报告输出路径（默认 `docs/audit/<date>-pr-review-<branch>.md`）

## 状态机

```
scanning → compile → static → security → test → integration(可跳过) → deep(可选) → ai(可选) → done / failed
```

任一步骤命令失败即 `failed` 并退出非零（fail-closed）；报告记录完整状态流转。

## 环节与严重度映射（全量 15 项门禁）

| # | 环节 | 检查 | 严重度映射 |
|---|------|------|-----------|
| 1 | diff 扫描 | `git diff --check`（空白/冲突标记） | whitespace-error → medium |
| 2 | 编译 | `cargo check --workspace --all-targets` | compile-error → **critical** |
| 3 | 静态 | `cargo fmt --all --check` | fmt → medium |
| 4 | 静态 | `cargo clippy --workspace --all-targets -D warnings` | compile-error → critical / lint-warning → medium |
| 5 | 静态 | `python scripts/check-unwrap.py`（铁律 2 裸 unwrap） | 生产 unwrap → **high** |
| 6 | 安全 | `sensitive-field-audit.js` | EXPOSED → **critical** |
| 7 | 一致性 | `feature-consistency.js` | 失败 → high |
| 8-10 | 一致性 | `doc-code-consistency.js` / `adr-code-consistency.js` / `assertion-value-check.js` | 失败 → low（不阻塞） |
| 11 | 测试 | `cargo test -p sz-rust-orm-facade -p sz-rust-sz300` | test-failure → **critical** |
| 12 | 集成 | `cargo test -p sz-rust-sz300 --test jobs_integration_test -- --ignored`（需 MySQL，`--skip-integration` 跳过） | integration-failure → high |
| 13 | 深验证（`--deep`） | `cargo-mutants`（变异杀率）+ `cargo-llvm-cov`（jobs.rs ≥75% 行覆盖） | mutation-killrate / coverage → high |
| 14 | AI（`--ai`） | OpenAI 兼容端点评审 diff + 问题清单 | 无 key / 请求失败 / 解析失败 → medium |

## 通过标准

- 状态机到达 `done` 且无 ≥ threshold 级别问题 → 退出码 0，报告 ✅ 通过
- 任一步骤失败或存在 ≥ threshold 问题 → 退出码非零，报告 ❌ 阻塞/失败
- 报告落盘 `docs/audit/`，含状态流转、问题清单（severity/file/rule/message）、补充信息（变更集 + AI 评审）、结论

## AI 评审 prompt 设计

对齐《代码审查 Skill 实战》ai_reviewer：把 diff（截断 8000 字符）+ 已发现问题清单喂给模型，要求输出 3-5 个最重要问题（性能/安全/可维护性/并发）+ 修改建议 + 1-10 评分，只输出 Markdown。AI 评审结论只进报告（不影响阻塞判定）。

## 已知边界

- AI 评审走 curl 直调（bash 编排层）；`sz-rust-ai-facade::llm::OpenAiProvider`（llm/openai.rs）为同协议的库实现，可配置 `base_url=https://ai.csdn.net/api/model/v1` + `model=glm_for_coding` 供 Rust 应用（如 sz300）接线使用
- 只审查静态资产，不运行测试（测试验证走 `sz-rust-test-coverage` / gauntlet）
