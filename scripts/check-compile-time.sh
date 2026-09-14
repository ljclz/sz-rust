#!/usr/bin/env bash
# 编译时间 CI 监控（拆包收益量化 + 防编译时间回归）
#
# 用法：
#   scripts/check-compile-time.sh [--save-baseline] [--threshold-percent 30] [--clean]
#
# 流程：
#   1. 对关键 crate（core + 7 facade + cli）逐个 `cargo check` 计时（wall-clock）
#   2. 输出各 crate 编译时长与总时长
#   3. 与 scripts/compile-time-baseline.json 对比，超阈值（默认 +30%）输出 warning
#
# --save-baseline：将本次测量写入基线文件（首次运行必须调用一次）
# --clean：先 cargo clean 再测量（本地首次建基线用；CI fresh checkout 无需）
#
# 注意：增量编译会显著降低测量值，CI 与基线对比应在同等新鲜度（clean）下进行。

set -euo pipefail

THRESHOLD_PERCENT=30
SAVE_BASELINE=false
DO_CLEAN=false
for arg in "$@"; do
  case "$arg" in
    --save-baseline) SAVE_BASELINE=true ;;
    --clean) DO_CLEAN=true ;;
    --threshold-percent=*) THRESHOLD_PERCENT="${arg#*=}" ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASELINE_FILE="$SCRIPT_DIR/compile-time-baseline.json"

# Python 探测：优先 python3（CI/Linux），回退 python（Windows 常见安装名）
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

# 关键 crate（覆盖全部 7 个 facade + core + cli）
CRATES=(
  sz-rust-core
  sz-rust-orm-facade
  sz-rust-http-facade
  sz-rust-cache-facade
  sz-rust-state-facade
  sz-rust-infra-facade
  sz-rust-auth-facade
  sz-rust-pay-facade
  sz-rust-cli
)

if [ "$DO_CLEAN" = true ]; then
  echo "▶ cargo clean（全量测量模式）…"
  cargo clean --quiet
fi

echo "▶ 逐个 crate 测量 cargo check 编译时间…"
measure_crate() {
  local name="$1"
  local start_ms end_ms elapsed_ms
  start_ms=$("$PYTHON" -c 'import time; print(int(time.time()*1000))')
  cargo check -p "$name" --quiet 2>/dev/null
  end_ms=$("$PYTHON" -c 'import time; print(int(time.time()*1000))')
  elapsed_ms=$((end_ms - start_ms))
  "$PYTHON" -c "print(round($elapsed_ms / 1000.0, 2))"
}

# 生成测量 JSON（shell 组装 + python 校验）
MEASUREMENTS="{"
TOTAL=0.0
for crate in "${CRATES[@]}"; do
  secs=$(measure_crate "$crate")
  TOTAL=$("$PYTHON" -c "print(round($TOTAL + $secs, 2))")
  MEASUREMENTS="${MEASUREMENTS}\"${crate}\": ${secs},"
  printf "  %-24s %6.2fs\n" "$crate" "$secs"
done
MEASUREMENTS="${MEASUREMENTS}\"__TOTAL__\": ${TOTAL}}"
printf "  %-24s %6.2fs\n" "__TOTAL__" "$TOTAL"

MEASURED_JSON=$(echo "$MEASUREMENTS" | "$PYTHON" -c "import json,sys; print(json.dumps(json.load(sys.stdin), indent=2, ensure_ascii=False))")

if [ "$SAVE_BASELINE" = true ]; then
  echo "$MEASURED_JSON" > "$BASELINE_FILE"
  echo "✅ 基线已保存：$BASELINE_FILE（总编译时间 ${TOTAL}s）"
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
    exit(0)  # 仅 warning，不阻塞 CI（环境差异会引入噪声）
print("✅ 所有 crate 编译时间均在基线阈值内")
PY
