#!/usr/bin/env bash
# sz300 迁移验证脚本（v1.4.0 T05）
#
# 用法：
#   scripts/sz300-migration-verify.sh --save-baseline   # 记录迁移前测试基线
#   scripts/sz300-migration-verify.sh --verify          # 迁移后对比基线
#   scripts/sz300-migration-verify.sh --help            # 帮助
#
# 功能：
#   --save-baseline：在 sz-rust-oss 仓库执行 cargo test -p sz-rust-sz300，记录用例数与通过率
#   --verify：在当前 workspace 执行 cargo test -p sz-rust-sz300，与基线对比

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE_FILE="$SCRIPT_DIR/sz300-test-baseline.json"

# sz-rust-oss 仓库路径（迁移前 sz300 所在位置）
OSS_REPO="${SZ_RUST_OSS_REPO:-E:/vue/test/鲜视达/rust/sz-rust-oss}"

PYTHON=""
for cand in python3 python; do
  if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import json' >/dev/null 2>&1; then
    PYTHON="$cand"
    break
  fi
done
if [ -z "$PYTHON" ]; then
  echo "❌ 需要 Python 3" >&2
  exit 1
fi

# 帮助
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  echo "用法: scripts/sz300-migration-verify.sh [--save-baseline] [--verify] [--help]"
  echo "  --save-baseline  记录迁移前测试基线（在 sz-rust-oss 仓库执行）"
  echo "  --verify         迁移后对比基线（在当前 workspace 执行）"
  echo ""
  echo "环境变量："
  echo "  SZ_RUST_OSS_REPO  sz-rust-oss 仓库路径（默认: $OSS_REPO）"
  exit 0
fi

# 解析测试输出，提取用例数和通过率
parse_test_output() {
  local output="$1"
  local total passed failed
  # cargo test 输出格式：test result: ok. N passed; M failed; ...
  local result_line
  result_line=$(echo "$output" | grep 'test result:' | tail -1)
  if [ -z "$result_line" ]; then
    echo '{"total": 0, "passed": 0, "failed": 0, "pass_rate": 0.0}'
    return
  fi
  total=$(echo "$result_line" | grep -oP '\d+ passed' | grep -oP '\d+' || echo 0)
  failed=$(echo "$result_line" | grep -oP '\d+ failed' | grep -oP '\d+' || echo 0)
  passed=$total
  total=$((passed + failed))
  local pass_rate
  if [ "$total" -gt 0 ]; then
    pass_rate=$("$PYTHON" -c "print(round($passed / $total * 100, 2))")
  else
    pass_rate=0.0
  fi
  echo "{\"total\": $total, \"passed\": $passed, \"failed\": $failed, \"pass_rate\": $pass_rate}"
}

if [ "${1:-}" = "--save-baseline" ]; then
  echo "▶ 记录迁移前测试基线（sz-rust-oss 仓库）…"
  if [ ! -d "$OSS_REPO" ]; then
    echo "❌ sz-rust-oss 仓库不存在：$OSS_REPO" >&2
    exit 1
  fi
  echo "  仓库路径：$OSS_REPO"
  echo "  执行 cargo test -p sz-rust-sz300 …"
  OUTPUT=$(cd "$OSS_REPO" && cargo test -p sz-rust-sz300 2>&1 || true)
  echo "$OUTPUT" | tail -5
  BASELINE=$(parse_test_output "$OUTPUT")
  echo "$BASELINE" | "$PYTHON" -m json.tool > "$BASELINE_FILE"
  echo "✅ 基线已保存：$BASELINE_FILE"
  "$PYTHON" -c "import json; d=json.load(open('$BASELINE_FILE')); print(f'  用例数: {d[\"total\"]}, 通过: {d[\"passed\"]}, 通过率: {d[\"pass_rate\"]}%')"
  exit 0
fi

if [ "${1:-}" = "--verify" ]; then
  echo "▶ 迁移后验证（当前 workspace）…"
  if [ ! -f "$BASELINE_FILE" ]; then
    echo "❌ 基线文件不存在，请先运行：scripts/sz300-migration-verify.sh --save-baseline" >&2
    exit 1
  fi
  echo "  执行 cargo test -p sz-rust-sz300 …"
  OUTPUT=$(cd "$ROOT_DIR" && cargo test -p sz-rust-sz300 2>&1 || true)
  echo "$OUTPUT" | tail -5
  CURRENT=$(parse_test_output "$OUTPUT")
  echo ""
  echo "▶ 与基线对比…"
  "$PYTHON" - "$BASELINE_FILE" "$CURRENT" <<'PY'
import json, sys
baseline = json.load(open(sys.argv[1]))
current = json.loads(sys.argv[2])

print(f"  基线：  用例数={baseline['total']}, 通过={baseline['passed']}, 通过率={baseline['pass_rate']}%")
print(f"  当前：  用例数={current['total']}, 通过={current['passed']}, 通过率={current['pass_rate']}%")

if current['total'] == baseline['total'] and current['pass_rate'] == baseline['pass_rate']:
    print("✅ 用例数一致、通过率一致")
    sys.exit(0)
elif current['total'] >= baseline['total'] and current['pass_rate'] >= baseline['pass_rate']:
    print("✅ 用例数 ≥ 基线且通过率 ≥ 基线（允许新增测试）")
    sys.exit(0)
else:
    print("❌ 测试回归！")
    sys.exit(1)
PY
  exit $?
fi

echo "用法: scripts/sz300-migration-verify.sh [--save-baseline] [--verify] [--help]"
exit 1