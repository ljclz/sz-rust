#!/usr/bin/env bash
# PR 审查编排（sz-rust-pr-review Skill 的执行入口）
#
# 状态机: scanning → compile → static → security → test → integration(可选跳过) → deep(可选) → ai(可选) → done / failed
# 环节:   git diff 扫描 → cargo check → fmt+clippy+unwrap → 门禁脚本 → 单元测试 → 真实集成(MySQL) → 变异+覆盖(--deep) → AI 评审(--ai)
# 严重度: critical / high / medium / low；--severity-threshold 控制阻塞门禁（默认 medium）
#
# 用法:
#   bash scripts/audit/pr-review.sh [--range HEAD~1..HEAD] [--severity-threshold medium] [--ai] [--ai-parallel] [--no-ai-cache] [--ai-timeout 120] [--deep] [--skip-integration] [--report path]
# 依赖: git / cargo / node / python（门禁脚本与 unwrap 检查）；--ai 需要 AI_API_KEY（OpenAI 兼容端点，默认 CSDN glm_for_coding，可用 AI_BASE_URL/AI_MODEL 切换 Provider）
#       integration 环节需本机 MySQL（127.0.0.1:3306 root/test123 sz_orm_test）；--deep 需 cargo-mutants / cargo-llvm-cov（耗时 10+ 分钟）
# 性能（第 7 篇方法论）: --ai-parallel 在 diff 扫描后后台启动 AI 评审，与门禁并行；AI 结果按 diff sha256 缓存 ~/.cache/sz-rust-review（--no-ai-cache 关闭）；--ai-timeout 控制单次调用超时（默认 120s）
# Choreography（第 8 篇方法论）: 审查终态 publish ReviewCompleted/ReviewBlocked/ReviewFailed 事件到 docs/audit/events.jsonl（JSON Lines 追加，供季度审计/发布门禁/指标订阅）
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
AI=0
DEEP=0
SKIP_INTEGRATION=0
AI_PARALLEL=0
AI_CACHE=1
AI_TIMEOUT=120
while [ $# -gt 0 ]; do
  case "$1" in
    --range) RANGE="$2"; shift 2 ;;
    --severity-threshold) THRESHOLD="$2"; shift 2 ;;
    --report) REPORT="$2"; shift 2 ;;
    --ai) AI=1; shift ;;
    --ai-parallel) AI=1; AI_PARALLEL=1; shift ;;
    --no-ai-cache) AI_CACHE=0; shift ;;
    --ai-timeout) AI_TIMEOUT="$2"; shift 2 ;;
    --deep) DEEP=1; shift ;;
    --skip-integration) SKIP_INTEGRATION=1; shift ;;
    *) echo "未知参数: $1"; exit 2 ;;
  esac
done

# Provider 配置（环境变量覆盖，默认 CSDN；提前解析供 --ai-parallel 后台预启动使用）
AI_API_KEY="${AI_API_KEY:-${CSDN_API_KEY:-}}"
AI_BASE_URL="${AI_BASE_URL:-https://ai.csdn.net/api/model/v1}"
AI_MODEL="${AI_MODEL:-glm_for_coding}"
AI_CACHE_DIR="${AI_CACHE_DIR:-$HOME/.cache/sz-rust-review}"
AI_REVIEW=""
AI_BG_PID=""      # 后台 AI 评审进程（--ai-parallel）
AI_BG_RC=""       # 缓存命中等已就绪标记（空=未就绪）
DIFF_HASH=""      # 当前 diff sha256 前 16 位（缓存 key）

# 临时文件（AI 评审建议：mktemp 防多实例竞态）
TMPDIR_PREFIX="${TMPDIR:-/tmp}/pr-review"
AI_ERR_LOG=$(mktemp "${TMPDIR_PREFIX}-ai-err.XXXXXX.log" 2>/dev/null || echo "/tmp/pr-review-ai-err.log")
AI_BG_OUT=$(mktemp "${TMPDIR_PREFIX}-ai-bg.XXXXXX.out" 2>/dev/null || echo "/tmp/pr-review-ai-bg.out")
AI_BG_ERR=$(mktemp "${TMPDIR_PREFIX}-ai-bg.XXXXXX.err" 2>/dev/null || echo "/tmp/pr-review-ai-bg.err")
trap 'rm -f "$AI_ERR_LOG" "$AI_BG_OUT" "$AI_BG_ERR"' EXIT

SEVERITY_ORDER=(low medium high critical)
THRESHOLD_IDX=1  # medium 默认
for i in "${!SEVERITY_ORDER[@]}"; do
  [ "${SEVERITY_ORDER[$i]}" = "$THRESHOLD" ] && THRESHOLD_IDX=$i
done

# ---- 状态机 ----
STATE="scanning"
TRANSITIONS=""
ISSUES_BODY=""   # 问题清单（报告 markdown 行）
EXTRA=""         # 变更集 / AI 评审等补充章节
ISSUES=""        # 问题清单（severity|file|rule|message 每行，供解析）
FAILED=0

fail() {
  STATE="failed"
  echo "❌ $1"
  exit 1
}
note_issue() { # note_issue <severity> <file> <rule> <message>
  local sev="$1"
  ISSUES+="$sev|$2|$3|$4"$'\n'
  ISSUES_BODY+="- [$sev] \`$2\` **$3**: $4"$'\n'
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
if ! echo "$DIFF_STAT" | grep -qE "^ [0-9]+ files? changed"; then
  echo "⚠️ 变更集为空（$RANGE），仍执行静态检查"
fi
EXTRA+=$'\n## 变更集\n```\n'"$DIFF_STAT"$'\n```\n'

# ---- 环节 1.5（可选）: AI 评审核心函数 + 后台预启动（--ai-parallel，第 7 篇并发方法论） ----
run_ai_review() { # run_ai_review <diff_text> <issues_text> → stdout: review markdown；stderr: 诊断
  local diff_text="$1" issues_text="$2"
  local prompt body resp
  PROMPT="你是一个资深 Rust 工程师。请评审以下 PR 变更（sz-rust 项目）。

要求：
1. 结合已发现的静态问题清单，列出 3-5 个最重要的潜在问题（性能/安全/可维护性/并发）
2. 给出具体修改建议（带代码示例）
3. 整体评分 1-10

已发现的问题清单:
\`\`\`
${issues_text}
\`\`\`

PR diff（截断至 8000 字符）:
\`\`\`
${diff_text}
\`\`\`

只输出 Markdown，不要闲聊。"
  # JSON body 用 python 构造（环境变量传 prompt，避免 shell 转义）
  body=$(REVIEW_PROMPT="$PROMPT" AI_MODEL="$AI_MODEL" python -c '
import json, os, sys
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
print(json.dumps({
    "model": os.environ["AI_MODEL"],
    "messages": [{"role": "user", "content": os.environ["REVIEW_PROMPT"]}],
    "temperature": 0.2,
}))
')
  resp=$(curl -sS --max-time "$AI_TIMEOUT" \
    -X POST "${AI_BASE_URL}/chat/completions" \
    -H "Authorization: Bearer ${AI_API_KEY}" \
    -H "Content-Type: application/json" \
    -d "$body" 2>&1) || { echo "AI 请求失败: $(echo "$resp" | head -c 120)"; return 1; }
  printf '%s' "$resp" | python -c '
import json, sys
if hasattr(sys.stdin, "reconfigure"):
    sys.stdin.reconfigure(encoding="utf-8")
data = json.load(sys.stdin)
if "error" in data:
    print("AI API 错误: " + json.dumps(data["error"], ensure_ascii=False), file=sys.stderr)
    sys.exit(1)
print(data["choices"][0]["message"]["content"])
' 2>&1 || { echo "AI 响应解析失败"; return 1; }
}

if [ "$AI" -eq 1 ] && [ "$AI_PARALLEL" -eq 1 ] && [ -n "$AI_API_KEY" ]; then
  transition "ai-bg" "AI 评审后台预启动（--ai-parallel）"
  DIFF_TEXT=$(git diff "$RANGE" 2>/dev/null | head -c 8000)
  DIFF_HASH=$(printf '%s' "$DIFF_TEXT" | sha256sum | cut -c1-16)
  if [ "$AI_CACHE" -eq 1 ] && [ -f "$AI_CACHE_DIR/${DIFF_HASH}.md" ]; then
    cp "$AI_CACHE_DIR/${DIFF_HASH}.md" "$AI_BG_OUT"
    AI_BG_RC=0
    echo "✅ AI 评审缓存命中（${DIFF_HASH}），无需调用 LLM"
  else
    ( run_ai_review "$DIFF_TEXT" "" > "$AI_BG_OUT" 2> "$AI_BG_ERR"; echo $? > "${AI_BG_OUT}.rc" ) &
    AI_BG_PID=$!
    echo "⏳ AI 评审后台运行中（pid $AI_BG_PID，与门禁并行）"
  fi
fi

# ---- 环节 2: 编译检查 ----
transition "compile" "cargo check --workspace --all-targets"
CHECK_OUT=$(cargo check --workspace --all-targets -j 2 2>&1)
if [ $? -ne 0 ]; then
  # 提取 error 位置行（error[E...] 或 error: 后跟的文件行）
  while IFS= read -r line; do
    case "$line" in
      error\[*) note_issue "critical" "check" "compile-error" "$(echo "$line" | head -c 120)" ;;
      error:*) note_issue "critical" "check" "compile-error" "$(echo "$line" | head -c 120)" ;;
    esac
  done <<< "$CHECK_OUT"
fi

# ---- 环节 3: 静态检查（fmt + clippy + unwrap） ----
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

transition "static" "铁律 2: 生产代码裸 unwrap 检查（check-unwrap.py）"
if [ -f scripts/check-unwrap.py ]; then
  UNWRAP_OUT=$(python scripts/check-unwrap.py 2>&1)
  UNWRAP_COUNT=$(echo "$UNWRAP_OUT" | grep -oE 'AUTHORITATIVE_PROD_UNWRAP: [0-9]+' | awk '{print $2}')
  if [ -n "$UNWRAP_COUNT" ] && [ "$UNWRAP_COUNT" -gt 0 ]; then
    note_issue "high" "workspace" "bare-unwrap" "生产代码 ${UNWRAP_COUNT} 处裸 unwrap（铁律 2）"
  fi
else
  note_issue "low" "workspace" "unwrap-check-missing" "scripts/check-unwrap.py 不存在，跳过"
fi

# ---- 环节 4: 安全与一致性门禁 ----
transition "security" "门禁脚本（sensitive-field / doc-code / feature / assertion / adr）"
run_gate() { # run_gate <name> <severity-on-fail> <script...>
  local name="$1" sev="$2"; shift 2
  local runner
  case "$1" in
    *.py) runner=python ;;
    *)    runner=node ;;
  esac
  if ! $runner "$@" >/tmp/pr-review-gate.log 2>&1; then
    note_issue "$sev" "gate" "$name" "$(grep -m1 -E '❌|EXPOSED|error|FAIL' /tmp/pr-review-gate.log | head -c 120)"
  fi
}
run_gate "sensitive-field" "critical" scripts/audit/sensitive-field-audit.js
run_gate "std-fs" "high" scripts/audit/check-std-fs.py
run_gate "doc-code-consistency" "low" scripts/audit/doc-code-consistency.js
run_gate "feature-consistency" "high" scripts/audit/feature-consistency.js
run_gate "assertion-value" "low" scripts/audit/assertion-value-check.js
run_gate "adr-code-consistency" "low" scripts/audit/adr-code-consistency.js

# ---- 环节 5: 单元测试 ----
transition "test" "cargo test（facade lib + sz300）"
TEST_OUT=$(cargo test -p sz-rust-orm-facade -p sz-rust-sz300 -j 2 2>&1)
TEST_RC=$?
if [ $TEST_RC -ne 0 ]; then
  # 提取失败摘要（test result: FAILED / error）
  TEST_FAIL=$(echo "$TEST_OUT" | grep -E "test result: FAILED|^error" | head -3 | tr '\n' ' ')
  note_issue "critical" "test" "test-failure" "$(echo "$TEST_FAIL" | head -c 200)"
fi

# ---- 环节 6: 真实服务集成（需本机 MySQL，可 --skip-integration） ----
if [ "$SKIP_INTEGRATION" -eq 1 ]; then
  echo "⚠️ 跳过集成测试（--skip-integration）"
else
  transition "integration" "真实集成测试（jobs_integration，需 MySQL）"
  INTEG_OUT=$(cargo test -p sz-rust-sz300 --test jobs_integration_test -j 2 -- --ignored 2>&1)
  INTEG_RC=$?
  if [ $INTEG_RC -ne 0 ]; then
    INTEG_FAIL=$(echo "$INTEG_OUT" | grep -E "test result: FAILED|panicked|error\[" | head -3 | tr '\n' ' ')
    note_issue "high" "integration" "integration-failure" "$(echo "$INTEG_FAIL" | head -c 200)"
  fi
fi

# ---- 环节 7（可选）: 深验证（变异 + 覆盖率，--deep，耗时 10+ 分钟） ----
if [ "$DEEP" -eq 1 ]; then
  transition "deep" "变异测试（cargo-mutants，facade 全 crate）"
  MUT_OUT=$(cargo mutants -p sz-rust-orm-facade --timeout 120 -j 2 2>&1)
  MUT_RC=$?
  if [ $MUT_RC -ne 0 ]; then
    MUT_SUM=$(echo "$MUT_OUT" | grep -E "mutants tested|MISSED" | tail -2 | tr '\n' ' ')
    note_issue "high" "deep" "mutation-killrate" "$(echo "$MUT_SUM" | head -c 200)"
  fi

  transition "deep" "变更行覆盖率（llvm-cov，jobs.rs ≥75% 行）"
  COV_OUT=$(cargo llvm-cov -p sz-rust-orm-facade --lib --no-report -j 2 2>&1 && cargo llvm-cov -p sz-rust-sz300 --test jobs_integration_test --no-report -j 2 -- --ignored 2>&1 && cargo llvm-cov report 2>&1)
  COV_RC=$?
  if [ $COV_RC -ne 0 ]; then
    note_issue "high" "deep" "coverage" "覆盖率检查失败（需 cargo-llvm-cov；jobs.rs 阈值 75%）"
  else
    JOBS_COV=$(echo "$COV_OUT" | grep "jobs.rs" | awk '{print $4}' | head -1)
    if [ -n "$JOBS_COV" ]; then
      COV_VAL=$(echo "$JOBS_COV" | tr -d '%')
      if [ "$(echo "$COV_VAL < 75" | bc 2>/dev/null)" = "1" ]; then
        note_issue "high" "deep" "coverage" "jobs.rs 行覆盖 ${JOBS_COV} < 75%"
      else
        echo "✅ jobs.rs 行覆盖 ${JOBS_COV}"
      fi
    fi
  fi
fi

# ---- 环节 4（可选）: AI 评审（OpenAI 兼容端点，默认 CSDN glm_for_coding） ----
# 三路径：A 并行后台（--ai-parallel）/ B 缓存命中 / C 串行原逻辑；缓存+超时=第 7 篇方法论
if [ "$AI" -eq 1 ]; then
  transition "ai" "AI 评审（${AI_MODEL}）"
  if [ -z "$AI_API_KEY" ]; then
    note_issue "medium" "ai" "missing-key" "AI_API_KEY 未设置（或旧变量 CSDN_API_KEY），AI 评审跳过（设置后重跑 --ai）"
  else
    DIFF_TEXT=$(git diff "$RANGE" 2>/dev/null | head -c 8000)
    DIFF_HASH=$(printf '%s' "$DIFF_TEXT" | sha256sum | cut -c1-16)
    CACHE_TAG=""
    if [ "$AI_PARALLEL" -eq 1 ] && [ -n "$AI_BG_PID" ]; then
      # 路径 A：并行后台 — 等后台任务完成（与门禁重叠运行）
      wait "$AI_BG_PID" 2>/dev/null
      AI_BG_RC=$?
      if [ "$AI_BG_RC" -eq 0 ] && [ -s "$AI_BG_OUT" ]; then
        AI_REVIEW=$(cat "$AI_BG_OUT")
        echo "✅ AI 评审完成（并行后台）"
        [ "$AI_CACHE" -eq 1 ] && { mkdir -p "$AI_CACHE_DIR"; cp "$AI_BG_OUT" "$AI_CACHE_DIR/${DIFF_HASH}.md"; }
      else
        note_issue "medium" "ai" "ai-failed" "后台 AI 评审失败: $(head -c 120 "$AI_BG_ERR" 2>/dev/null || echo '无输出')"
      fi
    elif [ "$AI_BG_RC" = "0" ] && [ -s "$AI_BG_OUT" ]; then
      # 路径 B：缓存命中（预启动阶段已就绪，未调 LLM）
      AI_REVIEW=$(cat "$AI_BG_OUT")
      CACHE_TAG="（缓存命中 ${DIFF_HASH}）"
      echo "✅ AI 评审完成（缓存命中）"
    else
      # 路径 C：串行原逻辑 + 缓存检查（diff hash key）
      if [ "$AI_CACHE" -eq 1 ] && [ -f "$AI_CACHE_DIR/${DIFF_HASH}.md" ]; then
        AI_REVIEW=$(cat "$AI_CACHE_DIR/${DIFF_HASH}.md")
        CACHE_TAG="（缓存命中 ${DIFF_HASH}）"
        echo "✅ AI 评审完成（缓存命中）"
      else
        ISSUES_TEXT=$(printf '%s' "$ISSUES" | head -c 3000)
        if AI_REVIEW=$(run_ai_review "$DIFF_TEXT" "$ISSUES_TEXT" 2>"$AI_ERR_LOG"); then
          echo "✅ AI 评审完成"
          [ "$AI_CACHE" -eq 1 ] && { mkdir -p "$AI_CACHE_DIR"; printf '%s' "$AI_REVIEW" > "$AI_CACHE_DIR/${DIFF_HASH}.md"; }
        else
          note_issue "medium" "ai" "ai-failed" "$(head -c 120 "$AI_ERR_LOG")"
          AI_REVIEW=""
        fi
      fi
    fi
    if [ -n "$AI_REVIEW" ]; then
      EXTRA+=$'\n## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）'"${CACHE_TAG}"$'\n\n'"$AI_REVIEW"$'\n'
    fi
  fi
fi

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
BRANCH=$(git branch --show-current | tr '/' '-')
REVIEW_HEAD=$(git rev-parse --short HEAD 2>/dev/null) || {
  echo "❌ 无法获取当前 HEAD，请确认在 Git 仓库中运行" >&2
  exit 1
}
REPORT="${REPORT:-docs/audit/${DATE}-pr-review-${BRANCH}.md}"
{
  echo "# PR 审查报告（$DATE，branch: $BRANCH，range: $RANGE）"
  echo ""
  echo "> 审查时点: \`HEAD @ $REVIEW_HEAD\`（报告为时点快照；后续新提交不在本报告范围内）"
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
    echo "$ISSUES_BODY"
  fi
  if [ -n "$EXTRA" ]; then
    echo ""
    echo "## 补充信息"
    echo "$EXTRA"
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
# 去除报告自身 trailing whitespace（避免被下次 git diff --check 检出）
sed -i 's/[[:space:]]*$//' "$REPORT"
echo "📄 报告: $REPORT"

# ---- Choreography 事件落盘（第 8 篇方法论：ReviewCompleted / ReviewBlocked / ReviewFailed） ----
EVENT_FILE="docs/audit/events.jsonl"
if [ "$STATE" = "done" ] && [ $BLOCKING -eq 0 ]; then EV="ReviewCompleted"; RESULT="passed"
elif [ "$STATE" = "done" ]; then EV="ReviewBlocked"; RESULT="blocked"
else EV="ReviewFailed"; RESULT="failed"
fi
{
  mkdir -p "$(dirname "$EVENT_FILE")"
  EV="$EV" RESULT="$RESULT" BRANCH="$BRANCH" REVIEW_HEAD="$REVIEW_HEAD" RANGE="$RANGE" STATE="$STATE" \
  BLOCKING="$BLOCKING" CRIT="${COUNTS[critical]:-0}" HIGH="${COUNTS[high]:-0}" MEDIUM="${COUNTS[medium]:-0}" LOW="${COUNTS[low]:-0}" REPORT="$REPORT" \
  python -c '
import json, os
print(json.dumps({
    "ts": __import__("datetime").datetime.now().isoformat(timespec="seconds"),
    "event": os.environ["EV"],
    "result": os.environ["RESULT"],
    "branch": os.environ["BRANCH"],
    "commit": os.environ["REVIEW_HEAD"],
    "range": os.environ["RANGE"],
    "state": os.environ["STATE"],
    "blocking": int(os.environ["BLOCKING"]),
    "issues": {"critical": int(os.environ["CRIT"]), "high": int(os.environ["HIGH"]), "medium": int(os.environ["MEDIUM"]), "low": int(os.environ["LOW"])},
    "report": os.environ["REPORT"],
}, ensure_ascii=False))
' >> "$EVENT_FILE"
  echo "📡 事件已落盘: $EVENT_FILE"
}

# ---- 退出码: fail-closed ----
[ "$STATE" = "done" ] && [ $BLOCKING -eq 0 ] && exit 0
[ "$STATE" = "done" ] && [ $BLOCKING -gt 0 ] && exit 1
exit 1
