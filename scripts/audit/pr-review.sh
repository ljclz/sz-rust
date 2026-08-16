#!/usr/bin/env bash
# PR 审查编排（sz-rust-pr-review Skill 的执行入口）
#
# 状态机: scanning → static → security → done / failed（任一步失败即 FAILED，退出非零）
# 环节:   git diff 扫描 → fmt+clippy → 门禁脚本 → 汇总报告
# 严重度: critical / high / medium / low；--severity-threshold 控制阻塞门禁（默认 medium）
#
# 用法:
#   bash scripts/audit/pr-review.sh --range HEAD~1..HEAD [--severity-threshold medium] [--report path]
# 依赖: git / cargo / node（门禁脚本）
# fail-closed: 任何环节命令失败或输出不可解析 → 该环节失败 → 整体退出非零

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# cargo 不在 PATH（Windows Git Bash 常见，同 .githooks/pre-commit 处理）
if ! command -v cargo >/dev/null 2>&1; then
  for dir in "$HOME/.cargo/bin" "$USERPROFILE/.cargo/bin"; do
    if [ -x "$dir/cargo" ] || [ -x "$dir/cargo.exe" ]; then
      export PATH="$dir:$PATH"
      break
    fi
  done
fi

# ---- 参数解析 ----
RANGE="HEAD~1..HEAD"
THRESHOLD="medium"
REPORT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --range) RANGE="$2"; shift 2 ;;
    --severity-threshold) THRESHOLD="$2"; shift 2 ;;
    --report) REPORT="$2"; shift 2 ;;
    *) echo "未知参数: $1"; exit 2 ;;
  esac
done

SEVERITY_ORDER=(low medium high critical)
THRESHOLD_IDX=1  # medium 默认
for i in "${!SEVERITY_ORDER[@]}"; do
  [ "${SEVERITY_ORDER[$i]}" = "$THRESHOLD" ] && THRESHOLD_IDX=$i
done

# ---- 状态机 ----
STATE="scanning"
TRANSITIONS=""
BODY=""          # 报告正文累积
ISSUES=""        # 问题清单（severity|file|rule|message 每行）
FAILED=0

fail() {
  STATE="failed"
  echo "❌ $1"
  exit 1
}
note_issue() { # note_issue <severity> <file> <rule> <message>
  local sev="$1"
  ISSUES+="$sev|$2|$3|$4"$'\n'
  BODY+="- [$sev] \`$2\` **$3**: $4"$'\n'
}
transition() {
  TRANSITIONS+="$STATE → $1; "
  STATE="$1"
  echo "▶ [$STATE] $2"
}

# ---- 环节 1: diff 扫描 ----
transition "scanning" "git diff 扫描（$RANGE）"
DIFF_STAT=$(git diff --stat "$RANGE" 2>&1) || fail "git diff 失败: $DIFF_STAT"
DIFF_CHECK=$(git diff --check "$RANGE" 2>&1)
if [ -n "$DIFF_CHECK" ]; then
  note_issue "medium" "diff" "whitespace-error" "空白/冲突标记错误: $(echo "$DIFF_CHECK" | head -3 | tr '\n' ' ')"
fi
if [ -z "$(echo "$DIFF_STAT" | sed '/^$/d' | tail -n +3)" ]; then
  echo "⚠️ 变更集为空（$RANGE），仍执行静态检查"
fi
BODY+=$'\n## 变更集\n```\n'"$DIFF_STAT"$'\n```\n'

# ---- 环节 2: 静态检查（fmt + clippy） ----
transition "static" "cargo fmt --all --check"
if ! cargo fmt --all --check >/tmp/pr-review-fmt.log 2>&1; then
  note_issue "medium" "workspace" "fmt" "格式不合格: $(head -2 /tmp/pr-review-fmt.log | tr '\n' ' ')"
fi

transition "static" "cargo clippy --workspace --all-targets -D warnings"
CLIPPY_OUT=$(cargo clippy --workspace --all-targets -j 2 -- -D warnings 2>&1)
CLIPPY_RC=$?
if [ $CLIPPY_RC -ne 0 ]; then
  # 提取 error/warning 行（含位置）
  while IFS= read -r line; do
    case "$line" in
      error:*) note_issue "critical" "clippy" "compile-error" "$(echo "$line" | head -c 120)" ;;
      warning:*) note_issue "medium" "clippy" "lint-warning" "$(echo "$line" | head -c 120)" ;;
    esac
  done <<< "$CLIPPY_OUT"
fi

# ---- 环节 3: 安全与一致性门禁 ----
transition "security" "门禁脚本（sensitive-field / doc-code / feature / assertion / adr）"
run_gate() { # run_gate <name> <severity-on-fail> <script...>
  local name="$1" sev="$2"; shift 2
  if ! node "$@" >/tmp/pr-review-gate.log 2>&1; then
    note_issue "$sev" "gate" "$name" "$(grep -m1 -E '❌|EXPOSED|error|FAIL' /tmp/pr-review-gate.log | head -c 120)"
  fi
}
run_gate "sensitive-field" "critical" scripts/audit/sensitive-field-audit.js
run_gate "doc-code-consistency" "low" scripts/audit/doc-code-consistency.js
run_gate "feature-consistency" "high" scripts/audit/feature-consistency.js
run_gate "assertion-value" "low" scripts/audit/assertion-value-check.js
run_gate "adr-code-consistency" "low" scripts/audit/adr-code-consistency.js

# ---- 汇总: 严重度计数与阻塞判定 ----
transition "done" "汇总报告"
declare -A COUNTS=([critical]=0 [high]=0 [medium]=0 [low]=0)
BLOCKING=0
while IFS='|' read -r sev file rule msg; do
  [ -z "$sev" ] && continue
  COUNTS[$sev]=$(( ${COUNTS[$sev]:-0} + 1 ))
  for i in "${!SEVERITY_ORDER[@]}"; do
    if [ "${SEVERITY_ORDER[$i]}" = "$sev" ] && [ $i -ge $THRESHOLD_IDX ]; then
      BLOCKING=$((BLOCKING + 1))
    fi
  done
done <<< "$ISSUES"

# ---- 写报告 ----
DATE=$(date +%Y-%m-%d)
BRANCH=$(git branch --show-current)
REPORT="${REPORT:-docs/audit/${DATE}-pr-review-${BRANCH}.md}"
{
  echo "# PR 审查报告（$DATE，branch: $BRANCH，range: $RANGE）"
  echo ""
  echo "## 状态机"
  echo "- ${TRANSITIONS}最终状态: **${STATE}**"
  echo "- 严重度阈值: $THRESHOLD（≥ 该级别阻塞）"
  echo ""
  echo "## 问题清单（${COUNTS[critical]} critical / ${COUNTS[high]} high / ${COUNTS[medium]} medium / ${COUNTS[low]} low）"
  echo ""
  if [ -z "$ISSUES" ]; then
    echo "✅ 未发现问题"
  else
    echo "$BODY"
  fi
  echo ""
  echo "## 结论"
  if [ "$STATE" = "done" ]; then
    if [ $BLOCKING -eq 0 ]; then
      echo "✅ 通过（无 ≥ $THRESHOLD 级别问题）"
    else
      echo "❌ **阻塞**: $BLOCKING 个 ≥ $THRESHOLD 级别问题，禁止合入"
    fi
  else
    echo "❌ **失败**: 流程中断于 $STATE"
  fi
} > "$REPORT"
echo "📄 报告: $REPORT"

# ---- 退出码: fail-closed ----
[ "$STATE" = "done" ] && [ $BLOCKING -eq 0 ] && exit 0
[ "$STATE" = "done" ] && [ $BLOCKING -gt 0 ] && exit 1
exit 1
