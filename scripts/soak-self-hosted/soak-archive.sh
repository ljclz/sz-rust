#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/config-defaults.sh"

REPORT_DIR="${DEFAULT_REPORT_DIR}"
INDEX_FILE=""
REPORT_PATH=""
RUN_ID=""
TRIGGER=""
DURATION=""
STATUS=""
QUERY=""
CLEANUP_EXPIRED=""
RETENTION_DAYS=30

while [[ $# -gt 0 ]]; do
    case $1 in
        --report-dir) REPORT_DIR="$2"; shift 2 ;;
        --report-path) REPORT_PATH="$2"; shift 2 ;;
        --run-id) RUN_ID="$2"; shift 2 ;;
        --trigger) TRIGGER="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --status) STATUS="$2"; shift 2 ;;
        --query) QUERY="$2"; shift 2 ;;
        --cleanup-expired) CLEANUP_EXPIRED="1"; shift ;;
        --retention-days) RETENTION_DAYS="$2"; shift 2 ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

INDEX_FILE="$REPORT_DIR/index.csv"
mkdir -p "$REPORT_DIR"

if [ ! -f "$INDEX_FILE" ]; then
    echo "run_id,trigger,trigger_time,set_duration,actual_duration,status,report_path,rust_version,retention_days" > "$INDEX_FILE"
fi

if [ -n "$QUERY" ]; then
    echo "--- 查询归档记录: $QUERY ---"
    head -1 "$INDEX_FILE"
    tail -n +2 "$INDEX_FILE" | grep -i "$QUERY" || echo "(无匹配记录)"
    exit 0
fi

if [ -n "$CLEANUP_EXPIRED" ]; then
    echo "--- 清理过期归档（保留 $RETENTION_DAYS 天）---"
    CUTOFF_DATE=$(date -d "-${RETENTION_DAYS} days" +%Y-%m-%d 2>/dev/null || true)
    if [ -n "$CUTOFF_DATE" ]; then
        find "$REPORT_DIR" -maxdepth 1 -type d -name "20*" | while read -r dir; do
            dir_date=$(basename "$dir")
            if [[ "$dir_date" < "$CUTOFF_DATE" ]]; then
                echo "删除过期归档: $dir"
                rm -rf "$dir"
            fi
        done
    fi
    echo "✅ 过期清理完成"
    exit 0
fi

if [ -z "$REPORT_PATH" ] || [ -z "$RUN_ID" ] || [ -z "$TRIGGER" ] || [ -z "$DURATION" ] || [ -z "$STATUS" ]; then
    echo "归档模式需要: --report-path --run-id --trigger --duration --status"
    exit 70
fi

ARCHIVE_DATE=$(date +%Y-%m-%d)
ARCHIVE_DIR="$REPORT_DIR/$ARCHIVE_DATE"
mkdir -p "$ARCHIVE_DIR"

ARCHIVED_FILE="$ARCHIVE_DIR/soak-report-${RUN_ID}.csv"

if [ -f "$REPORT_PATH" ]; then
    cp "$REPORT_PATH" "$ARCHIVED_FILE"
    echo "✅ 报告已归档: $ARCHIVED_FILE"
else
    echo "⚠️ 报告文件不存在: $REPORT_PATH，创建空报告"
    echo "timestamp,rss_mb,fd_count,ops_per_sec,p99_latency_ms,error_count" > "$ARCHIVED_FILE"
fi

TRIGGER_TIME=$(date +"%Y-%m-%d %H:%M:%S")
RUST_VERSION=$(rustc --version 2>/dev/null | grep -oP '\d+\.\d+\.\d+' | head -1 || echo "unknown")

echo "$RUN_ID,$TRIGGER,$TRIGGER_TIME,$DURATION,$DURATION,$STATUS,$ARCHIVED_FILE,$RUST_VERSION,$RETENTION_DAYS" >> "$INDEX_FILE"

echo "✅ 索引已更新: $INDEX_FILE"
exit 0
