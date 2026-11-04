#!/usr/bin/env bash
# 编译时间 CI 监控（v1.4.0 T04：全 41 crate + 分阶段报告 + 趋势追踪）
#
# 用法：
#   scripts/check-compile-time.sh [--save-baseline] [--threshold-percent 10] [--clean] [--phases]
#
# 流程：
#   1. 对全 workspace crate 逐个 `cargo check` 计时（wall-clock）
#   2. 输出各 crate 编译时长与总时长
#   3. 与 scripts/compile-time-baseline.json 对比，超阈值（默认 +10%）输出 warning
#   4. --phases：输出分阶段（依赖解析/编译/链接）时间明细
#
# --save-baseline：将本次测量写入基线文件
# --clean：先 cargo clean 再测量
# --phases：输出分阶段时间明细

set -euo pipefail

THRESHOLD_PERCENT=10
SAVE_BASELINE=false
DO_CLEAN=false
SHOW_PHASES=false
for arg in "$@"; do
  case "$arg" in
    --save-baseline) SAVE_BASELINE=true ;;
    --clean) DO_CLEAN=true ;;
    --phases) SHOW_PHASES=true ;;
    --threshold-percent=*) THRESHOLD_PERCENT="${arg#*=}" ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE_FILE="$SCRIPT_DIR/compile-time-baseline.json"

PYTHON=""
for cand in python3 python; do
  if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import json' >/dev/null 2>&1; then
    PYTHON="$cand"
    break
  fi
done
if [ -z "$PYTHON" ]; then
  echo "❌ 需要 Python 3（json 模块）" >&2
  exit 1
fi

# 全 workspace crate 列表（v1.4.0 T04.1：从 9 扩展至 41）
CRATES=(
  sz-rust-core
  sz-rust-http-facade
  sz-rust-cache-facade
  sz-rust-state-facade
  sz-rust-infra-facade
  sz-rust-auth-facade
  sz-rust-pay-facade
  sz-rust-orm-facade
  sz-rust-orm-ext-facade
  sz-rust-router-facade
  sz-rust-middleware-facade
  sz-rust-mvc-facade
  sz-rust-mcp
  sz-rust-facade-tests
  sz-rust-macros
  sz-rust-examples
  sz-rust-addon-example
  sz-rust-addons-loader
  sz-rust-cli
  sz-rust-observability
  sz-rust-ai-facade
  sz-rust-capability
  sz-rust-rag
  sz-rust-frontend-codegen
  sz-rust-tracing
  sz-rust-pdf
  sz-rust-workflow
  sz-rust-wasm
  sz-rust-vector-db
  sz-rust-marketplace
  sz-rust-visual
  sz-rust-addons-admin
  sz-rust-facade
  sz-rust-config-center
  sz-rust-service-registry
  sz-rust-distributed-tx
  sz-rust-api-gateway
  sz-rust-codegen-loop
  sz-rust-testkit
  sz-rust-ops-api
  sz-rust
)

now_ms() { "$PYTHON" -c 'import time; print(int(time.time()*1000))'; }

if [ "$DO_CLEAN" = true ]; then
  echo "▶ cargo clean（全量测量模式）…"
  cargo clean --quiet
fi

# 分阶段时间测量（T04.3）
if [ "$SHOW_PHASES" = true ]; then
  echo "▶ 分阶段时间测量…"
  t0=$(now_ms)
  cargo metadata --no-deps --format-version 1 > /dev/null 2>&1
  t1=$(now_ms)
  DEP_MS=$((t1 - t0))
  echo "  依赖解析阶段：$(echo "scale=2; $DEP_MS / 1000" | bc 2>/dev/null || "$PYTHON" -c "print(round($DEP_MS/1000.0, 2))")s"
fi

echo "▶ 逐个 crate 测量 cargo check 编译时间（${#CRATES[@]} crates）…"

measure_crate() {
  local name="$1"
  local start_ms end_ms elapsed_ms
  start_ms=$(now_ms)
  cargo check -p "$name" --quiet 2>/dev/null
  end_ms=$(now_ms)
  elapsed_ms=$((end_ms - start_ms))
  "$PYTHON" -c "print(round($elapsed_ms / 1000.0, 2))"
}

MEASUREMENTS="{"
TOTAL=0.0
for crate in "${CRATES[@]}"; do
  secs=$(measure_crate "$crate")
  TOTAL=$("$PYTHON" -c "print(round($TOTAL + $secs, 2))")
  MEASUREMENTS="${MEASUREMENTS}\"${crate}\": ${secs},"
  printf "  %-28s %6.2fs\n" "$crate" "$secs"
done
MEASUREMENTS="${MEASUREMENTS}\"__TOTAL__\": ${TOTAL}}"
printf "  %-28s %6.2fs\n" "__TOTAL__" "$TOTAL"

if [ "$SHOW_PHASES" = true ]; then
  echo ""
  echo "── 分阶段时间明细 ──"
  echo "  依赖解析：$("$PYTHON" -c "print(round($DEP_MS/1000.0, 2))")s"
  echo "  编译检查：${TOTAL}s"
  echo "  链接：N/A（cargo check 不执行链接）"
  echo "  总计：$("$PYTHON" -c "print(round($TOTAL + $DEP_MS/1000.0, 2))")s"
fi

MEASURED_JSON=$(echo "$MEASUREMENTS" | "$PYTHON" -c "import json,sys; print(json.dumps(json.load(sys.stdin), indent=2, ensure_ascii=False))")

if [ "$SAVE_BASELINE" = true ]; then
  echo "$MEASURED_JSON" > "$BASELINE_FILE"
  echo "✅ 基线已保存：$BASELINE_FILE（总编译时间 ${TOTAL}s，${#CRATES[@]} crates）"
  exit 0
fi

if [ ! -f "$BASELINE_FILE" ]; then
  echo "⚠️ 基线文件不存在，请先运行：scripts/check-compile-time.sh --save-baseline --clean"
  echo "$MEASURED_JSON"
  exit 1
fi

echo "▶ 与基线对比（阈值：+${THRESHOLD_PERCENT}%）…"
"$PYTHON" - "$BASELINE_FILE" "$THRESHOLD_PERCENT" <<PY
import json, sys
baseline = json.load(open(sys.argv[1], encoding="utf-8"))
measured = json.loads("""$MEASURED_JSON""")
threshold = float(sys.argv[2])

issues = []
for name, base_time in baseline.items():
    cur = measured.get(name)
    if cur is None:
        continue
    delta = (cur - base_time) / base_time * 100 if base_time > 0 else 0.0
    flag = "⚠️" if delta > threshold else "✅"
    print(f"  {flag} {name}: {base_time}s → {cur}s ({delta:+.0f}%)")
    if delta > threshold:
        issues.append(name)

if issues:
    print("::warning::编译时间回归: " + ", ".join(issues))
    exit(0)
print("✅ 所有 crate 编译时间均在基线阈值内")
PY
