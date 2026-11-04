#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/config-defaults.sh"

REPORT_DIR="${DEFAULT_REPORT_DIR}"
PROTECTED_PORT="${DEFAULT_PROTECTED_PORT}"
SOAK_PORTS="${DEFAULT_SOAK_PORTS}"
CRON_MARKER="${DEFAULT_CRON_MARKER}"

while [[ $# -gt 0 ]]; do
    case $1 in
        --report-dir) REPORT_DIR="$2"; shift 2 ;;
        --protected-port) PROTECTED_PORT="$2"; shift 2 ;;
        --soak-ports) SOAK_PORTS="$2"; shift 2 ;;
        --cron-marker) CRON_MARKER="$2"; shift 2 ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

echo "=== 6h Soak 验证脚本 ==="
echo "Report Dir: $REPORT_DIR"
echo "Protected Port: $PROTECTED_PORT"
echo "Soak Ports: $SOAK_PORTS"
echo "Cron Marker: $CRON_MARKER"
echo ""

FAIL_COUNT=0

check_consecutive_success() {
    echo "--- 检查 1: 连续 2 次 6h soak 成功 ---"
    INDEX_FILE="$REPORT_DIR/index.csv"
    if [ ! -f "$INDEX_FILE" ]; then
        echo "❌ 索引文件不存在: $INDEX_FILE"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        return
    fi
    SUCCESS_COUNT=$(tail -n +2 "$INDEX_FILE" | { grep "6h" || true; } | { grep "success" || true; } | wc -l)
    SUCCESS_COUNT=$(echo "$SUCCESS_COUNT" | tr -d '[:space:]')
    if [ "$SUCCESS_COUNT" -ge 2 ]; then
        echo "✅ 找到 $SUCCESS_COUNT 次 6h soak 成功记录"
        tail -n +2 "$INDEX_FILE" | grep "6h" | grep "success" | tail -2 || true
    else
        echo "⚠️ 仅找到 $SUCCESS_COUNT 次 6h soak 成功记录（需 2 次）"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

check_cron_config() {
    echo "--- 检查 2: cron 配置正确性 ---"
    CRON_ENTRIES=$(crontab -l 2>/dev/null | grep "$CRON_MARKER" 2>/dev/null || true)
    ENTRY_COUNT=$(echo "$CRON_ENTRIES" | grep -c . 2>/dev/null || true)
    ENTRY_COUNT=${ENTRY_COUNT:-0}
    if [ "$ENTRY_COUNT" -eq 2 ]; then
        echo "✅ 找到 2 条 cron 规则"
        echo "$CRON_ENTRIES"
    else
        echo "❌ 找到 $ENTRY_COUNT 条 cron 规则（需 2 条）"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

check_protected_process() {
    echo "--- 检查 3: 被保护进程存活 ---"
    PID=$(fuser ${PROTECTED_PORT}/tcp 2>/dev/null | tr -d ' ' || true)
    if [ -n "$PID" ]; then
        echo "✅ 端口 $PROTECTED_PORT 有进程监听 (PID=$PID)"
    else
        echo "❌ 端口 $PROTECTED_PORT 无进程监听"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

check_port_residue() {
    echo "--- 检查 4: 采样端口无残留 ---"
    RESIDUE=""
    if [[ "$SOAK_PORTS" == *-* ]]; then
        START="${SOAK_PORTS%%-*}"
        END="${SOAK_PORTS##*-}"
        for ((p=START; p<=END; p++)); do
            PID=$(fuser ${p}/tcp 2>/dev/null | tr -d ' ' || true)
            if [ -n "$PID" ]; then
                RESIDUE="$RESIDUE $p(PID=$PID)"
            fi
        done
    fi
    if [ -z "$RESIDUE" ]; then
        echo "✅ 采样端口 $SOAK_PORTS 无残留进程"
    else
        echo "❌ 端口残留:$RESIDUE"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

check_consecutive_success
echo ""
check_cron_config
echo ""
check_protected_process
echo ""
check_port_residue
echo ""

echo "=== 验证结果 ==="
if [ "$FAIL_COUNT" -eq 0 ]; then
    echo "✅ 6h soak 验证通过（4/4 检查通过）"
    exit 0
else
    echo "❌ 6h soak 验证失败（$FAIL_COUNT 项检查未通过）"
    exit 1
fi