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
- `--report <path>`：报告输出路径（默认 `docs/audit/<date>-pr-review-<branch>.md`）

## 状态机

```
scanning → static → security → ai(可选) → done / failed
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
| AI（`--ai`） | CSDN glm_for_coding 评审 diff + 问题清单 | 无 key / 请求失败 / 解析失败 → medium |

## 通过标准

- 状态机到达 `done` 且无 ≥ threshold 级别问题 → 退出码 0，报告 ✅ 通过
- 任一步骤失败或存在 ≥ threshold 问题 → 退出码非零，报告 ❌ 阻塞/失败
- 报告落盘 `docs/audit/`，含状态流转、问题清单（severity/file/rule/message）、补充信息（变更集 + AI 评审）、结论

## AI 评审 prompt 设计

对齐《代码审查 Skill 实战》ai_reviewer：把 diff（截断 8000 字符）+ 已发现问题清单喂给模型，要求输出 3-5 个最重要问题（性能/安全/可维护性/并发）+ 修改建议 + 1-10 评分，只输出 Markdown。AI 评审结论只进报告（不影响阻塞判定）。

## 已知边界

- AI 评审走 curl 直调（bash 编排层）；`sz-rust-ai-facade::llm::OpenAiProvider`（llm/openai.rs）为同协议的库实现，可配置 `base_url=https://ai.csdn.net/api/model/v1` + `model=glm_for_coding` 供 Rust 应用（如 sz300）接线使用
- 只审查静态资产，不运行测试（测试验证走 `sz-rust-test-coverage` / gauntlet）
