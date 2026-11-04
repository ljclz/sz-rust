#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/config-defaults.sh"

ACTION="${1:-setup}"
CRON_MARKER="${DEFAULT_CRON_MARKER}"
SOAK_RUNNER="${DEFAULT_SOAK_RUNNER}"
WORK_DIR="${DEFAULT_WORK_DIR}"

shift 2>/dev/null || true

while [[ $# -gt 0 ]]; do
    case $1 in
        --cron-marker) CRON_MARKER="$2"; shift 2 ;;
        --soak-runner) SOAK_RUNNER="$2"; shift 2 ;;
        --work-dir) WORK_DIR="$2"; shift 2 ;;
        setup|remove) ACTION="$1"; shift ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

case "$ACTION" in
    setup)
        echo "=== 配置 Soak Test cron 调度 ==="
        echo "Cron Marker: $CRON_MARKER"
        echo "Soak Runner: $SOAK_RUNNER"
        echo "Work Dir: $WORK_DIR"

        (crontab -l 2>/dev/null | grep -v "$CRON_MARKER" || true) > /tmp/crontab-new

        echo "0 0 * * 0 TZ=UTC bash $SOAK_RUNNER --duration 6h --trigger cron --work-dir $WORK_DIR $CRON_MARKER weekly" >> /tmp/crontab-new

        echo "0 18 * * * TZ=UTC bash $SOAK_RUNNER --duration 6h --trigger nightly --work-dir $WORK_DIR $CRON_MARKER nightly" >> /tmp/crontab-new

        crontab /tmp/crontab-new
        rm -f /tmp/crontab-new

        echo "✅ cron 调度已配置"
        echo "--- 当前 crontab ---"
        crontab -l | grep "$CRON_MARKER"
        ;;

    remove)
        echo "=== 移除 Soak Test cron 调度 ==="
        echo "Cron Marker: $CRON_MARKER"
        (crontab -l 2>/dev/null | grep -v "$CRON_MARKER" || true) > /tmp/crontab-new
        crontab /tmp/crontab-new
        rm -f /tmp/crontab-new
        echo "✅ cron 调度已移除"
        ;;

    *)
        echo "用法: soak-cron-setup.sh [setup|remove] [--cron-marker MARKER] [--soak-runner PATH] [--work-dir DIR]"
        exit 1
        ;;
esac
