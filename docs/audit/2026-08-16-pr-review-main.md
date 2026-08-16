# PR 审查报告（2026-08-16，branch: main，range: HEAD~1..HEAD）

## 状态机
- scanning → scanning; scanning → static; static → static; static → security; security → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 0 low）

✅ 未发现问题

## 补充信息

## 变更集
```
 .trae/skills/sz-rust-pr-review/SKILL.md            |  10 +-
 CHANGELOG.md                                       |   3 +-
 docs/audit/2026-08-16-pr-review-main.md            | 122 ++++++++++++++++++++-
 ...211\247\350\241\214\346\214\207\345\215\227.md" |  17 ++-
 scripts/audit/pr-review.sh                         |  42 ++++---
 5 files changed, 164 insertions(+), 30 deletions(-)
```

## AI 评审



## PR 评审报告（sz-rust / scripts/audit/pr-review.sh）

### 最重要的问题

#### 1. [兼容性] Python `reconfigure` 无版本保护 — **Medium**

`sys.stdout.reconfigure(encoding="utf-8")` 仅在 **Python 3.7+** 可用。若 CI 镜像仍为 Python 3.6（部分企业环境），会直接抛出 `AttributeError` 导致整个 AI 评审流程崩溃，且错误信息不直观。

**建议修改：**
```bash
# 方案 A：shell 层统一设置，不依赖 Python 版本
AI_BODY=$(PYTHONIOENCODING=utf-8 REVIEW_PROMPT="$PROMPT" python -c '...')

# 方案 B：Python 内做版本检查
python -c '
import sys
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
# ...
'
```

---

#### 2. [并发安全] 临时文件路径硬编码，存在竞态条件 — **Medium**

`/tmp/pr-review-ai-err.log` 是固定路径。在并发 CI 场景下（多个 branch/PR 同时触发），不同 job 会互相覆盖日志文件，导致错误信息错乱或丢失。

**建议修改：**
```bash
# 使用 mktemp 生成唯一临时文件
AI_ERR_LOG=$(mktemp /tmp/pr-review-ai-err.XXXXXX.log)
trap "rm -f '$AI_ERR_LOG'" EXIT

# 后续引用 $AI_ERR_LOG 替代硬编码路径
```

---

#### 3. [可观测性] AI API 错误信息未进入报告 — **Medium**

当 AI Provider 返回错误时，错误信息只写入 stderr 并 `exit 1`，但**未被捕获到最终报告中**。审查者只能看到 "AI 评审失败" 而无法得知原因（如本次的 `Account suspended by risk control`），必须手动翻查临时日志文件。

**建议修改：**
```bash
if AI_REVIEW=$(printf '%s' "$AI_RESP" | python -c '...' 2>"$AI_ERR_LOG"); then
  EXTRA+=$'\n## AI 评审\n\n'"$AI_REVIEW"$'\n'
else
  AI_ERR=$(cat "$AI_ERR_LOG" 2>/dev/null || echo "未知错误")
  EXTRA+=$'\n## AI 评审\n\n⚠️ AI 评审失败: '"$AI_ERR"$'\n'
fi
```

---

#### 4. [健壮性] 变更集检测正则过于宽松 — **Low**

```bash
if ! echo "$DIFF_STAT" | grep -qE "[0-9]+ files? changed"; then
```

该正则过于宽松。若某个被修改的文件路径中恰好包含 "1 file changed" 字符串（极端情况），会导致误判。

**建议修改：**
```bash
# 匹配 git diff --stat 的标准输出格式（行首开始匹配）
if ! echo "$DIFF_STAT" | grep -qE "^[[:space:]]*[0-9]+ files? changed"; then
```

---

#### 5. [可维护性] 新旧环境变量优先级未明确 — **Low**

`CSDN_API_KEY`（旧）与 `AI_API_KEY`（新）共存时，如果用户同时设置两者，优先级逻辑不明确，可能导致意外行为。

**建议修改：**
```bash
# 新变量优先，旧变量降级兼容
API_KEY="${AI_API_KEY:-$CSDN_API_KEY}"
BASE_URL="${AI_BASE_URL:-https://ai.csdn.net/api/model/v1}"
MODEL="${AI_MODEL:-glm_for_coding}"
```

---

### 整体评分：**7/10**

**优点：**
- OpenAI 兼容 Provider 通用化设计合理，扩展性好
- 错误信息增强（输出 Provider 原始 error 详情）提升了可观测性
- Windows GBK 编码 bug 修复是必要的
- AI 结论只进报告不影响阻塞判定，fail-closed 策略正确

**待改进：**
- 临时文件竞态、Python 版本兼容性、错误信息入报告这三项是实际运行中会触发的稳定性问题，建议合入前修复
- 文档和 CHANGELOG 更新完整，状态机描述清晰


## 结论
✅ 通过（无 ≥ medium 级别问题）
