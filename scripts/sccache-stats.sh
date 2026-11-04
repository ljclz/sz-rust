#!/usr/bin/env bash
# sccache 缓存统计（v1.4.0 T01.2）
#
# 用法：
#   scripts/sccache-stats.sh [--json] [--reset]
#
# --json  : 输出原始 JSON（供 CI 解析）
# --reset : 重置统计计数器后退出
#
# 依赖：sccache 已安装且在 PATH 中

set -euo pipefail

OUTPUT_JSON=false
DO_RESET=false
for arg in "$@"; do
  case "$arg" in
    --json)  OUTPUT_JSON=true ;;
    --reset) DO_RESET=true ;;
    --help|-h)
      echo "用法: scripts/sccache-stats.sh [--json] [--reset]"
      echo "  --json   输出原始 JSON（供 CI 解析）"
      echo "  --reset  重置统计计数器后退出"
      exit 0
      ;;
  esac
done

if ! command -v sccache >/dev/null 2>&1; then
  echo "❌ sccache 未安装，请参考 docs/developer/sccache-guide.md 安装" >&2
  exit 1
fi

if [ "$DO_RESET" = true ]; then
  sccache --zero-stats
  echo "✅ 统计计数器已重置"
  exit 0
fi

# 获取 JSON 格式统计
STATS_JSON=$(sccache --show-stats --stats-format json)

if [ "$OUTPUT_JSON" = true ]; then
  echo "$STATS_JSON"
  exit 0
fi

# Python 探测
PYTHON=""
for cand in python3 python; do
  if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import json' >/dev/null 2>&1; then
    PYTHON="$cand"
    break
  fi
done
if [ -z "$PYTHON" ]; then
  echo "⚠️  需要 Python 3 解析 JSON，回退到原始输出" >&2
  sccache --show-stats
  exit 0
fi

# 解析并格式化输出
echo "$STATS_JSON" | "$PYTHON" - <<'PY'
import json, sys

data = json.load(sys.stdin)
stats = data.get("stats", data)

requests       = stats.get("get_requests", 0)
cache_hits     = stats.get("cache_hits", 0)
cache_misses   = stats.get("cache_misses", 0)
errors         = stats.get("errors", 0)
non_cacheable  = stats.get("non_cacheable", 0)
compilations   = stats.get("compilations", 0)

hit_rate = (cache_hits / requests * 100) if requests > 0 else 0.0

print("┌─────────────────────────────────────┐")
print("│       sccache 缓存统计报告          │")
print("├─────────────────────────────────────┤")
print(f"│  总请求数      : {requests:>10}        │")
print(f"│  缓存命中      : {cache_hits:>10}        │")
print(f"│  缓存未命中    : {cache_misses:>10}        │")
print(f"│  不可缓存      : {non_cacheable:>10}        │")
print(f"│  编译错误      : {errors:>10}        │")
print(f"│  实际编译      : {compilations:>10}        │")
print("├─────────────────────────────────────┤")
print(f"│  命中率        : {hit_rate:>9.1f}%       │")
print("└─────────────────────────────────────┘")

if requests > 0 and hit_rate >= 70:
    print(f"✅ 命中率 {hit_rate:.1f}% ≥ 70%（达标）")
elif requests > 0:
    print(f"⚠️  命中率 {hit_rate:.1f}% < 70%（建议执行两次编译后复测）")
else:
    print("ℹ️  暂无统计数据，请先执行一次编译")
PY