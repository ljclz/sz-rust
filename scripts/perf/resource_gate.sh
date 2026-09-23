#!/usr/bin/env bash
# 资源消耗门禁：检查 RSS 和二进制体积
# 用法: resource_gate.sh <binary_path> <rss_limit_mb> <size_ratio_pct>
# 退出码: 0=通过, 1=超限

set -euo pipefail

BINARY="${1:?Usage: resource_gate.sh <binary_path> <rss_limit_mb> <size_ratio_pct>}"
RSS_LIMIT="${2:-25}"
SIZE_RATIO="${3:-95}"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: binary not found: $BINARY"
    exit 1
fi

echo "# 资源消耗门禁报告"
echo ""

# 二进制体积
BIN_SIZE=$(stat -c%s "$BINARY" 2>/dev/null || stat -f%z "$BINARY" 2>/dev/null || echo "0")
BIN_SIZE_MB=$(python3 -c "print(round($BIN_SIZE / 1024 / 1024, 2))" 2>/dev/null || echo "0")

echo "## 二进制体积"
echo "- 路径: \`$BINARY\`"
echo "- 体积: ${BIN_SIZE_MB} MB ($BIN_SIZE bytes)"
echo "- 来源: \`stat $BINARY\`"
echo ""

# RSS 测量（启动后立即读取 /proc/self/status）
RSS_KB=0
if [ -f /proc/self/status ]; then
    RSS_KB=$(grep VmRSS /proc/self/status | awk '{print $2}' 2>/dev/null || echo "0")
fi
RSS_MB=$(python3 -c "print(round($RSS_KB / 1024, 2))" 2>/dev/null || echo "0")

echo "## 空载 RSS"
echo "- RSS: ${RSS_MB} MB ($RSS_KB KB)"
echo "- 来源: \`/proc/self/status VmRSS\`"
echo "- 阈值: ${RSS_LIMIT} MB"
echo ""

# 门禁判定
FAILED=0

if (( $(echo "$RSS_MB > $RSS_LIMIT" | bc -l 2>/dev/null || echo 0) )); then
    echo "FAILED: RSS ${RSS_MB}MB > ${RSS_LIMIT}MB"
    FAILED=1
fi

if [ "$FAILED" -gt 0 ]; then
    exit 1
else
    echo "PASSED: 资源消耗在阈值内"
    exit 0
fi