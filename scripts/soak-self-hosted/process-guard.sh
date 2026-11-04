#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/config-defaults.sh"

ACTION=""
PROTECTED_PORT="${DEFAULT_PROTECTED_PORT}"
SOAK_DIR="${DEFAULT_WORK_DIR}"
PROTECTED_PROCESS="${DEFAULT_PROTECTED_PROCESS}"
RESTART_SCRIPT="${DEFAULT_RESTART_SCRIPT}"

while [[ $# -gt 0 ]]; do
    case $1 in
        --action) ACTION="$2"; shift 2 ;;
        --protected-port) PROTECTED_PORT="$2"; shift 2 ;;
        --soak-dir) SOAK_DIR="$2"; shift 2 ;;
        --protected-process) PROTECTED_PROCESS="$2"; shift 2 ;;
        --restart-script) RESTART_SCRIPT="$2"; shift 2 ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

PROTECTED_PIDS_FILE="$SOAK_DIR/protected-pids.txt"
SOAK_PIDS_FILE="$SOAK_DIR/soak-pids.txt"

mkdir -p "$SOAK_DIR"

case "$ACTION" in
    pre-check)
        echo "--- 进程隔离预检查 ---"
        PID=$(fuser ${PROTECTED_PORT}/tcp 2>/dev/null | tr -d ' ' || true)
        if [ -z "$PID" ]; then
            PID=$(lsof -i :${PROTECTED_PORT} -t 2>/dev/null | head -1 || true)
        fi
        if [ -n "$PID" ]; then
            echo "$PID" > "$PROTECTED_PIDS_FILE"
            echo "✅ 端口 $PROTECTED_PORT PID=$PID 已记录保护"
        else
            echo "⚠️ 端口 $PROTECTED_PORT 无进程监听（$PROTECTED_PROCESS 可能未运行）"
            echo "" > "$PROTECTED_PIDS_FILE"
        fi
        exit 0
        ;;

    post-check)
        echo "--- 进程隔离后检查 ---"
        if [ ! -f "$PROTECTED_PIDS_FILE" ]; then
            echo "⚠️ 保护 PID 文件不存在，跳过检查"
            exit 0
        fi
        PROTECTED_PID=$(cat "$PROTECTED_PIDS_FILE" | tr -d ' ')
        if [ -z "$PROTECTED_PID" ]; then
            echo "⚠️ 无保护 PID，跳过检查"
            exit 0
        fi
        if kill -0 "$PROTECTED_PID" 2>/dev/null; then
            echo "✅ 保护进程 PID=$PROTECTED_PID 仍存活"
            exit 0
        else
            echo "❌ 保护进程 PID=$PROTECTED_PID 已死亡！尝试重启 $PROTECTED_PROCESS..."
            if [ -f "$RESTART_SCRIPT" ]; then
                bash "$RESTART_SCRIPT" || true
            fi
            exit 60
        fi
        ;;

    cleanup)
        echo "--- 清理 soak 相关进程 ---"
        if [ -f "$SOAK_PIDS_FILE" ]; then
            while read -r PID; do
                if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
                    echo "终止 soak 进程 PID=$PID"
                    kill "$PID" 2>/dev/null || true
                    sleep 2
                    kill -9 "$PID" 2>/dev/null || true
                fi
            done < "$SOAK_PIDS_FILE"
            rm -f "$SOAK_PIDS_FILE"
        fi
        echo "✅ soak 进程清理完成"
        exit 0
        ;;

    *)
        echo "用法: process-guard.sh --action <pre-check|post-check|cleanup> [--protected-port 8300] [--soak-dir DIR] [--protected-process NAME] [--restart-script PATH]"
        exit 1
        ;;
esac
