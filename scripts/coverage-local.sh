#!/usr/bin/env bash
#
# 本地一键覆盖率测量脚本
#
# 用法：
#   scripts/coverage-local.sh -p sz-rust-core --threshold 30
#   scripts/coverage-local.sh --changed --threshold 85
#   scripts/coverage-local.sh -p sz-rust-sz300 --threshold 85 --ignored

set -euo pipefail

CARGO="${CARGO:-cargo}"
THRESHOLD=85
CRATE=""
CHANGED=false
IGNORED=false
OUTPUT_DIR="target/coverage"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -p) CRATE="$2"; shift 2 ;;
    --threshold) THRESHOLD="$2"; shift 2 ;;
    --changed) CHANGED=true; shift ;;
    --ignored) IGNORED=true; shift ;;
    -h|--help)
      echo "用法: scripts/coverage-local.sh -p <crate> [--threshold <N>] [--changed] [--ignored]"
      exit 0 ;;
    *) echo "未知参数: $1"; exit 1 ;;
  esac
done

if ! command -v "$CARGO" &>/dev/null; then
  echo "错误: cargo 未找到"
  exit 127
fi

if ! "$CARGO" llvm-cov --version &>/dev/null 2>&1; then
  echo "错误: cargo-llvm-cov 未安装"
  echo "安装: cargo install cargo-llvm-cov"
  exit 127
fi

mkdir -p "$OUTPUT_DIR"

run_coverage() {
  local crate="$1"
  local xml_path="$OUTPUT_DIR/cobertura-${crate}.xml"
  local extra_args=""
  if $IGNORED; then
    extra_args="-- --ignored"
  fi
  echo "测量 $crate (门槛 ${THRESHOLD}%)..."
  "$CARGO" llvm-cov -p "$crate" \
    --cobertura --output-path "$xml_path" \
    --fail-under-lines "$THRESHOLD" \
    $extra_args 2>&1 || true
  echo "报告: $xml_path"
  if [ -f "$xml_path" ]; then
    node scripts/audit/per-crate-coverage.js --xml "$xml_path" --threshold "$THRESHOLD" 2>/dev/null || true
  fi
}

if $CHANGED; then
  CHANGED_CRATES=$(git diff --name-only HEAD~1 2>/dev/null | grep '^packages/' | cut -d/ -f2 | sort -u || true)
  if [ -z "$CHANGED_CRATES" ]; then
    echo "无变更 crate"
    exit 0
  fi
  for crate in $CHANGED_CRATES; do
    run_coverage "$crate"
  done
elif [ -n "$CRATE" ]; then
  run_coverage "$CRATE"
else
  echo "错误: 请指定 -p <crate> 或 --changed"
  exit 1
fi