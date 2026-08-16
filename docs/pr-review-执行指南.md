# PR 审查执行指南（sz-rust-pr-review）

> **执行入口**：`scripts/audit/pr-review.sh`
> **Skill 说明**：`.trae/skills/sz-rust-pr-review/SKILL.md`
> **适用版本**：2026-08-16（含 AI 评审环节，提交 `7c53bc2`）
> **项目根目录**：`e:\vue\test\鲜视达\rust\sz-rust`

---

## 一、用途

一次命令跑完提交/PR 变更集的全部质量检查（diff 扫描 → 静态检查 → 安全门禁 → 可选 AI 评审），生成带状态机与严重度模型的汇总报告。替代逐条手动执行各门禁脚本。

## 二、前置条件

| 依赖 | 说明 |
|------|------|
| git / cargo / node | 已有（门禁脚本依赖 node，静态检查依赖 cargo） |
| AI_API_KEY（可选） | 仅 `--ai` 需要；OpenAI 兼容 Provider 的密钥（默认 CSDN，可用 `AI_BASE_URL`/`AI_MODEL` 切换，如快手 `https://wanqing.streamlakeapi.com/api/gateway/coding/v1` + `KAT-Coder-Pro-V2.5`）；旧变量名 `CSDN_API_KEY` 兼容 |

> 无 key 也能跑静态审查；`--ai` 无 key 时如实记录 `missing-key`（medium）问题，不静默。

## 三、命令速查

```bash
# 1. 静态审查（默认审查最近一次提交）
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD

# 2. 静态 + AI 评审（默认 CSDN glm_for_coding）
export AI_API_KEY=sk-xxx
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai

# 2b. AI 评审（切换快手 Provider）
export AI_API_KEY=fPJ... AI_BASE_URL=https://wanqing.streamlakeapi.com/api/gateway/coding/v1 AI_MODEL=KAT-Coder-Pro-V2.5
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai

# 3. 审查整个分支相对 main 的改动
bash scripts/audit/pr-review.sh --range main...HEAD --ai

# 4. 提高阻塞阈值（只让 high/critical 阻塞，medium 不阻塞）
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --severity-threshold high

# 5. 指定报告输出路径
bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --report docs/audit/my-review.md
```

## 四、参数说明

| 参数 | 默认 | 说明 |
|------|------|------|
| `--range <git-range>` | `HEAD~1..HEAD` | 审查范围；PR 场景用 `main...HEAD` |
| `--severity-threshold <low\|medium\|high\|critical>` | `medium` | 阻塞阈值：≥ 该级别的问题导致审查失败（退出码非零、禁止合入） |
| `--ai` | 关 | 启用 AI 评审环节 |
| `--report <path>` | `docs/audit/<日期>-pr-review-<分支>.md` | 报告输出路径 |

## 五、状态机与环节

```
scanning → static → security → ai(可选) → done / failed
```

任一步骤命令失败即 `failed` 并退出非零（fail-closed），报告记录完整状态流转。

| 环节 | 检查内容 | 严重度映射 |
|------|---------|-----------|
| diff 扫描 | `git diff --check`（空白/冲突标记） | whitespace-error → medium |
| 静态 | `cargo fmt --all --check` | fmt → medium |
| 静态 | `cargo clippy --workspace --all-targets -D warnings` | compile-error → **critical** / lint-warning → medium |
| 安全 | `sensitive-field-audit.js`（密钥/脱敏） | EXPOSED → **critical** |
| 一致性 | `feature-consistency.js` | 失败 → high |
| 一致性 | `doc-code-consistency.js` / `adr-code-consistency.js` / `assertion-value-check.js` | 失败 → low（不阻塞） |
| AI | CSDN `glm_for_coding` 评审 diff + 问题清单 | 无 key / 请求失败 / 解析失败 → medium |

## 六、AI 评审环节

- **协议**：OpenAI 兼容（`POST {AI_BASE_URL}/chat/completions`，Bearer 鉴权），任意兼容端点可切换
- **默认端点（CSDN）**：`https://ai.csdn.net/api/model/v1` + `model=glm_for_coding`（套餐计费，底层 glm-5.2，上限 200k token；勿填 `glm-5.1`/`glm-5.2` 以免按普通模型计费）
- **已验证 Provider（快手）**：`https://wanqing.streamlakeapi.com/api/gateway/coding/v1` + `KAT-Coder-Pro-V2.5`（2026-08-16 实测通过，评审输出含推理过程）
- **Prompt 设计**：diff（截断 8000 字符）+ 已发现问题清单 → 要求输出 3-5 个最重要问题（性能/安全/可维护性/并发）+ 修改建议 + 1-10 评分，只输出 Markdown
- **输出位置**：报告"补充信息 → AI 评审"章节；**不影响阻塞判定**（AI 结论仅供参考）

## 七、报告说明

报告生成到 `docs/audit/<日期>-pr-review-<分支名>.md`，结构：

```
# PR 审查报告（日期，branch，range）
## 状态机        ← 状态流转 + 最终状态 + 阻塞阈值
## 问题清单      ← severity/file/rule/message 逐条（critical/high/medium/low 计数）
## 补充信息      ← 变更集 diff --stat + AI 评审（如有）
## 结论          ← ✅ 通过 / ❌ 阻塞（N 个 ≥ 阈值问题）/ ❌ 失败（流程中断）
```

**退出码**：`0` = 通过（done 且无 ≥ 阈值问题）；`1` = 阻塞或失败。

## 八、常见问题

| 现象 | 原因与处理 |
|------|-----------|
| 报告出现 `missing-key`（medium） | `AI_API_KEY`（或旧 `CSDN_API_KEY`）未设置或拼写错误；export 后重跑 `--ai` |
| 报告出现 `compile-error`（critical） | 工作区存在编译失败（可能是并行开发中的未完成代码）；修复后重跑 |
| 提示"变更集为空" | `--range` 范围内无改动；换范围（如 `main...HEAD`） |
| 报告显示 `failed` | 某环节命令失败（如 git range 非法）；按输出定位 |
| 只想快速看结果不想等 clippy | 无跳过选项（fail-closed 设计）；临时缩小 `--range` 不减少检查项 |

## 九、与其他门禁的关系

| 门禁 | 触发时机 | 与 pr-review 的关系 |
|------|---------|-------------------|
| `.githooks/pre-commit` | 每次 git commit | 3 项快查（fmt/clippy/unwrap），pr-review 是其超集 |
| `scripts/audit/jobs-gauntlet.sh` | 任务队列验证 | 8 层深验证（含变异/覆盖），pr-review 不含测试运行 |
| `scripts/audit/*.js` 门禁 | CI/手动 | pr-review 逐条调用并做严重度归一化 |

**建议流程**：开发中靠 pre-commit 快查；提交后、合入前跑一次 `pr-review.sh --ai`；任务队列等关键模块变更再跑 gauntlet。
